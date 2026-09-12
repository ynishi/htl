-- htl prelude: thin Lua-side layer over tl.lua (Teal compiler).
-- Loaded into every htl Lua state after `tl` is registered in package.preload.

local tl = require("tl")
local lint = require("htl.lint")
local fmt_mod = require("htl.fmt")
local H = {}

-- Source beats declaration. tl's own search order is `.d.tl` across the whole path
-- first, then `.tl`, so a stale `mods/defs.d.tl` written by a host would shadow the
-- `src/defs.tl` it was made from wherever the two sit on the path. htl's run-time
-- searchers already try `.tl` before `.d.tl`; make the checker agree, so a declaration
-- is what you check against only when no source of that module is reachable.
-- (`require_module` looks `tl.search_module` up on each call, so wrapping it works.)
do
   local tl_search = tl.search_module
   tl.search_module = function(module_name, search_all)
      local found, fd, tried = tl_search(module_name, false) -- `.tl` only
      if found or not search_all then
         return found, fd, tried
      end
      return tl_search(module_name, true) -- `.d.tl`, then `.lua`
   end
end

-- Rule -> on, for the rules lint.lua implements. Set by the Rust side, which resolves the
-- `+rule,-rule` spec against the registry (`lint::RULES`) and hands over the answer; the
-- state gets the defaults that way too, at construction. Nothing here has a rule list of
-- its own, which is what keeps the names a project may write and the names that run the
-- same set.
H.lint_cfg = {}

-- The same, for Teal's own warning kinds, keyed by the name htl reports them under
-- (`tl:hint`, `tl:unused`, ...). They are produced by the checker rather than by lint.lua,
-- so the selection is consulted where `result.warnings` is collected — see `warnings_of`.
H.tl_cfg = {}

function H.set_lints(cfg, tl_cfg)
   H.lint_cfg = cfg
   H.tl_cfg = tl_cfg or {}
end

-- Dependency name -> true, for the deps the project installed. Set by the Rust side from
-- the lockfile that `Htl::apply_project` links the entries from, so what is here is what
-- `require` can actually reach — a name in the manifest that nothing installed would be a
-- `module not found` a lint has no business talking around.
--
-- Empty for a run with no project (the proc macros, `htl run` on a loose file), which is
-- what a rule reading it should treat as "no". Rules that name a library ask this; nothing
-- about a project's own modules is in it.
H.deps = {}

-- Takes the names as a list and keeps the set, so that the table is built in this state:
-- the prelude is not always in the state its caller holds (`Htl::with_checker`), and a
-- table made on the other side of that line cannot be passed across it.
function H.set_deps(names)
   local deps = {}
   for _, name in ipairs(names or {}) do deps[name] = true end
   H.deps = deps
end

-- The rules lint.lua implements, for the test that holds this list to the registry.
function H.lint_rules()
   return lint.rule_names()
end

-- Format a file's source. Returns formatted text, or nil, err.
function H.format(filename, indent)
   local fd, err = io.open(filename, "rb")
   if not fd then return nil, "could not open " .. filename .. ": " .. tostring(err) end
   local src = fd:read("a")
   fd:close()
   return fmt_mod.format(src, filename, { indent = indent })
end

H.GEN_TARGET = "5.4"

local function new_env()
   local env = assert(tl.new_env({
      defaults = {
         feat_lax = "off",
         gen_compat = "off",
         gen_target = H.GEN_TARGET,
      },
   }), "htl: tl.new_env failed")
   -- Type report (symbols + resolved types by position): lets lints ask "what is the
   -- type of this expression" instead of guessing from literals.
   env.report_types = true
   return env
end

-- `---@struct` / `---@optional`: which records are built whole, and which of their fields
-- are exempt. The markers are comments, so the checker discards them; they are read back
-- from the source of the file that *declares* the record, which is rarely the file being
-- checked (a mod builds a record its SDK declares).
--
-- Reading them here rather than in lint.lua is deliberate. The rule needs to know what
-- type a bare `{ ... }` is being built as, and that is type information; lint.lua parses
-- afresh precisely so it never touches the checker's annotated tree (see the note on
-- L.run). So the type side is answered here and handed over as a lookup, the way
-- `subject_enum` already is.
-- Declaring files are read per check, not cached across checks. A cache that outlives the
-- run is a copy of a file that may since have been written, which is the failure `htl fix`
-- had when it verified against the store instead of the file it had just written.
local function source_lines(cache, file)
   local cached = cache[file]
   if cached ~= nil then return cached end
   local fd = io.open(file, "rb")
   if not fd then
      cache[file] = false
      return false
   end
   local src = fd:read("a")
   fd:close()
   local lines = {}
   for l in (src .. "\n"):gmatch("(.-)\n") do lines[#lines + 1] = l end
   cache[file] = lines
   return lines
end

-- On the line itself (trailing), or on the line above it when that line is nothing but
-- markers (own line). The line above is not read when it is a declaration of its own: a
-- trailing `---@optional` belongs to the field it trails, and reading it from the next
-- line too made `a: string   ---@optional` exempt `b` as well — one marker, two fields,
-- and `struct-fields` silent about the second. Same rule as `marker_on` below and as
-- `contract.rs`'s reader, which had it from the start.
local function has_marker(lines, y, marker)
   if not lines or not y then return false end
   local pat = "%-%-%-@" .. marker .. "%f[%W]"
   local l = lines[y]
   if l and l:match(pat) then return true end
   local above = lines[y - 1]
   return (above and above:match("^%s*%-%-%-@") and above:match(pat)) and true or false
end

local function indent_of(line)
   return #(line:match("^(%s*)") or "")
end

-- Fields of the record declared at `file:y`: a map of name -> the line it is declared on,
-- and the same fields in declaration order carrying the type each is written with.
-- A source scan: the record's fields are the `name: type` lines between the declaration
-- and the `end` that closes it, which is the first `end` indented no deeper than the
-- declaration itself. The map answers "is this one marked `---@optional`"; the order and
-- the type are what a fix spells at a construction site, and neither survives the
-- checker's own view of the record (`t.fields` is a hash, and a type there is a resolved
-- object rather than the words the author wrote).
local function field_lines(lines, y)
   local at, order = {}, {}
   local decl = lines[y]
   if not decl then return at, order end
   local base = indent_of(decl)
   for i = y + 1, #lines do
      local l = lines[i]
      if l:match("^%s*end%f[%W]") and indent_of(l) <= base then break end
      local name, written = l:match("^%s*([%w_]+)%s*:%s*(.-)%s*$")
      if name then
         at[name] = i
         -- `inflicts: string   ---@optional` is the type up to the comment.
         local ty = written:gsub("%s*%-%-.*$", "")
         order[#order + 1] = { name = name, type = ty }
      end
   end
   return at, order
end

-- What `struct_at` returns for a position that holds a `---@struct` record:
-- { name = "MonsterDef", required = { id = true, hp = true },
--   fields = { { name = "id", type = "string" }, { name = "hp", type = "integer" } } }.
local function struct_spec(cache, t)
   local lines = source_lines(cache, t.file)
   if not lines then return nil end
   if not has_marker(lines, t.y, "struct") then return nil end
   local at, order = field_lines(lines, t.y)
   local required, declared = {}, {}
   local any = false
   for name in pairs(t.fields or {}) do
      declared[name] = true
      if not has_marker(lines, at[name], "optional") then
         required[name] = true
         any = true
      end
   end
   if not any then return nil end
   -- `fields` is the required ones in declaration order: a fix that spells them at the
   -- site says them in the order a reader finds them in the declaration. It is filtered
   -- through the checker's field set, so a line the scan picked up from a nested record
   -- is not mistaken for one of this record's own.
   local fields, seen = {}, {}
   for _, f in ipairs(order) do
      if required[f.name] and not seen[f.name] then
         seen[f.name] = true
         fields[#fields + 1] = f
      end
   end
   -- `declared` as well as `required`: a key the literal sets that the record does not
   -- declare is how a misspelling looks from here, and the optional fields are declared.
   return { name = t.str or "record", required = required, declared = declared, fields = fields }
end

-- Resolver for the `struct-fields` lint: the `---@struct` record being built at (y, x),
-- or nil. One report per file, shared with `subject_enum_resolver` above.
local function struct_resolver(result, filename)
   local ok, report = pcall(tl.get_types, result)
   if not ok or type(report) ~= "table" then return nil end
   local by_pos = report.by_pos and report.by_pos[filename]
   if not by_pos then return nil end
   local specs, sources = {}, {}
   local function deref(id, depth)
      local t = report.types[id]
      if t and t.ref and depth < 8 then return deref(t.ref, depth + 1) end
      return t
   end
   return function(y, x)
      local id = by_pos[y] and by_pos[y][x]
      if not id then return nil end
      local t = deref(id, 0)
      if not t or not t.fields or not t.file or not t.y then return nil end
      if specs[id] == nil then specs[id] = struct_spec(sources, t) or false end
      return specs[id] or nil
   end
end

-- `---@sealed`: which records are built only where they are declared, and which functions
-- (`---@sealed(gate.judge, gate.open)`) may build them. Read from the declaring file, like
-- the `---@struct` markers above and for the same reason: the checker discards comments,
-- and the file being checked is rarely the one that declares the record.
--
-- The same position rule as `has_marker`: the line itself, or the line above when it is
-- only a marker comment. A record nested directly under `record Judged   ---@sealed` sits
-- on that line, and a marker read from any line above would seal the nested one too.
-- `marker_on` differs from `has_marker` in what it returns — the marker's argument list
-- as well — and is the reader `---@sealed` and `---@extensible` (below) share.
local function marker_args(line, marker)
   if not line then return false, nil end
   local at = line:find("%-%-%-@" .. marker .. "%f[%W]")
   if not at then return false, nil end
   local args = line:sub(at):match("^%-%-%-@" .. marker .. "%s*(%b())")
   return true, args and args:sub(2, -2) or nil
end

local function marker_on(lines, y, marker)
   if not lines or not y then return false, nil end
   local found, args = marker_args(lines[y], marker)
   if found then return true, args end
   local above = lines[y - 1]
   if above and above:match("^%s*%-%-%-@") then return marker_args(above, marker) end
   return false, nil
end

-- What `sealed_at` returns for a position whose type is a `---@sealed` record:
-- { name = "gate.Judged", file = "gate.tl", here = false, fns = { "gate.judge" } }.
-- `file` is the declaring file by its own name: the message says where the record may be
-- built, and where that file sits on this machine is not part of the answer.
local function clean_path(p)
   local s = tostring(p or ""):gsub("\\", "/")
   s = s:gsub("^%./", "")
   return s:lower()
end

local function same_file(a, b)
   if not a or not b then return false end
   local x, y = clean_path(a), clean_path(b)
   if x == y then return true end
   -- One side may be absolute and the other relative to the same root.
   return x:sub(-#y - 1) == "/" .. y or y:sub(-#x - 1) == "/" .. x
end

-- The record as its declaration names it: `Judged` nested in `record gate` is
-- `gate.Judged`, which is how a site that requires the module spells it. The checker's own
-- name for it is the bare `Judged` (the same gap `enum-cast` closes by reading the site).
-- Read from the lines above: the enclosing record is the first line indented less than the
-- declaration, and a line that is not a record declaration ends the nesting.
local function qualified_name(lines, y, name)
   local base = indent_of(lines[y] or "")
   if base == 0 then return name end
   local parts = { name }
   for i = y - 1, 1, -1 do
      local l = lines[i]
      if l and l:match("%S") and indent_of(l) < base then
         local outer = l:match("^%s*local%s+record%s+([%w_]+)") or l:match("^%s*record%s+([%w_]+)")
         if not outer then break end
         table.insert(parts, 1, outer)
         base = indent_of(l)
         if base == 0 then break end
      end
   end
   return table.concat(parts, ".")
end

local function sealed_spec(cache, t, filename)
   local lines = source_lines(cache, t.file)
   if not lines then return nil end
   local found, args = marker_on(lines, t.y, "sealed")
   if not found then return nil end
   local fns
   if args then
      fns = {}
      for name in args:gmatch("[^,%s]+") do fns[#fns + 1] = name end
      if #fns == 0 then fns = nil end
   end
   return {
      name = qualified_name(lines, t.y, t.str or "record"),
      file = tostring(t.file):match("([^/\\]+)$") or tostring(t.file),
      here = same_file(t.file, filename),
      fns = fns,
   }
end

-- Resolver for the `sealed-record` lint: the `---@sealed` record whose type is at (y, x)
-- -- the type a table constructor is built as, or the one an `as` cast lands on -- or nil.
-- Which of the two a position holds is the rule's business; both are the same question
-- here, and it is the same position report `struct_at` and `cast_at` answer from.
local function sealed_resolver(result, filename)
   local ok, report = pcall(tl.get_types, result)
   if not ok or type(report) ~= "table" then return nil end
   local by_pos = report.by_pos and report.by_pos[filename]
   if not by_pos then return nil end
   local specs, sources = {}, {}
   local function deref(id, depth)
      local t = report.types[id]
      if t and t.ref and depth < 8 then return deref(t.ref, depth + 1) end
      return t
   end
   return function(y, x)
      local id = by_pos[y] and by_pos[y][x]
      if not id then return nil end
      local t = deref(id, 0)
      if not t or not t.fields or not t.file or not t.y then return nil end
      if specs[id] == nil then specs[id] = sealed_spec(sources, t, filename) or false end
      return specs[id] or nil
   end
end

-- `---@nilable`: functions whose first return value may be nil. Teal cannot say it in the
-- type — every type there accepts nil, and `T | nil`, which the checker does act on, is
-- narrowed by `is` and `as` alone, so declaring it would turn `if v then` and `v or d`
-- into errors at every call site. The marker says it beside the declaration instead, the
-- type stays `T`, and the `nil-return` lint holds the one shape that cannot be right:
-- indexing the call directly.
--
-- Read from the declaring file like the markers above, and answered from the same position
-- report. The function's own type carries `file` and `y` — a record field declared
-- `parent: function(string): string` reports the field's line, and a `local function f`
-- its own — so the marker is read off that line with no detour through the record that
-- holds the field [measured: `tl.get_types`' `by_pos` at the callee position of
-- `p.parent(x)` gives `file=types/p.d.tl y=2`, the line `parent:` is on].
local function nilable_at_decl(cache, t)
   local lines = source_lines(cache, t.file)
   if not lines then return false end
   return (marker_on(lines, t.y, "nilable")) and true or false
end

-- Resolver for the `nil-return` lint: true when the function being called at (y, x) is
-- declared `---@nilable`. The position is the callee's — the variable in `f(x)`, the `.`
-- expression in `p.parent(x)` — and both hold the function's type.
--
-- A type with fields is a record and not a function: the marker means nothing on one, and
-- refusing it here keeps a `---@nilable` written by mistake on a record from reporting
-- every call of a field of it.
local function nilable_resolver(result, filename)
   local ok, report = pcall(tl.get_types, result)
   if not ok or type(report) ~= "table" then return nil end
   local by_pos = report.by_pos and report.by_pos[filename]
   if not by_pos then return nil end
   local marked, sources = {}, {}
   local function deref(id, depth)
      local t = report.types[id]
      if t and t.ref and depth < 8 then return deref(t.ref, depth + 1) end
      return t
   end
   return function(y, x)
      local id = by_pos[y] and by_pos[y][x]
      if not id then return false end
      local t = deref(id, 0)
      if not t or t.fields or not t.file or not t.y then return false end
      if marked[id] == nil then marked[id] = nilable_at_decl(sources, t) end
      return marked[id]
   end
end

-- `---@extensible`: records a table may carry keys beyond the ones they declare. Every
-- Teal record is closed, and a value arriving from outside the program is where that
-- costs: a mod written against a newer SDK, a save file from a later version, a table a
-- host will grow next release, each carrying one key more than the declaration knows
-- about. The marker says the declaration is not the whole set, and the only thing it
-- buys is that tl's `unknown field <k>` is dropped for the keys it does not declare.
--
-- Read from the declaring file, in both forms, like the markers above and for the same
-- reason: the checker discards comments, and the file being checked is rarely the one
-- that declares the record.
--
-- Nothing here relaxes which *declared* fields a literal must set. `---@struct` and
-- `---@required` are answered elsewhere and are untouched by this, so a record can be
-- open at one end (keys nobody declared) and closed at the other (fields it does).
local function extensible_declared(cache, t)
   local lines = source_lines(cache, t.file)
   if not lines then return nil end
   if not marker_on(lines, t.y, "extensible") then return nil end
   local declared = {}
   for name in pairs(t.fields or {}) do declared[name] = true end
   return declared
end

local function literal_key(item)
   local key = item.key
   if type(key) ~= "table" then return nil end
   -- `name = 1` parses as a string key whose `tk` is quoted; `["name"] = 1` reaches the
   -- checker the same way. `conststr` is what the checker itself keys the field by.
   if key.conststr then return key.conststr end
   if key.kind == "string" and type(key.tk) == "string" then return key.tk:sub(2, -2) end
   return nil
end

-- Where tl's `unknown field <k>` is the marker working rather than a mistake: `out[y:x]`
-- is the key named there, for every key an `---@extensible` record does not declare that
-- a literal built as one sets. Keyed by the table item's own position, which is where tl
-- raises it (`add_in_context(node[i], node, "unknown field " .. ck)` in the vendored
-- compiler), so the drop is on position and message and nothing wider.
--
-- The type side is answered the way `struct_at` answers it -- `tl.get_types`' `by_pos` at
-- the literal's position -- and the keys are read off the checker's own tree, since this
-- runs where the lint pass may not (a dependency's errors, and the resolver's
-- `gen_string` at the run boundary, both reach `collect_errors` with no lints asked for).
local function extensible_keys(filename, result)
   local out = {}
   if not result or not result.ast then return out end
   local ok, report = pcall(tl.get_types, result)
   if not ok or type(report) ~= "table" then return out end
   local by_pos = report.by_pos and report.by_pos[filename]
   if not by_pos then return out end
   local declareds, sources = {}, {}
   local function deref(id, depth)
      local t = report.types[id]
      if t and t.ref and depth < 8 then return deref(t.ref, depth + 1) end
      return t
   end
   local function declared_at(y, x)
      local id = by_pos[y] and by_pos[y][x]
      if not id then return nil end
      local t = deref(id, 0)
      if not t or not t.fields or not t.file or not t.y then return nil end
      if declareds[id] == nil then declareds[id] = extensible_declared(sources, t) or false end
      return declareds[id] or nil
   end
   local seen = {}
   local function go(n)
      if type(n) ~= "table" or seen[n] then return end
      seen[n] = true
      if n.kind == "literal_table" and n.y and n.x then
         local declared = declared_at(n.y, n.x)
         if declared then
            for _, item in ipairs(n) do
               local name = type(item) == "table" and item.y and item.x and literal_key(item)
               if name and not declared[name] then
                  out[item.y .. ":" .. item.x] = name
               end
            end
         end
      end
      for k, v in pairs(n) do
         if k ~= "if_parent" and k ~= "type" and k ~= "newtype" and k ~= "decltuple" and k ~= "expected"
            and type(v) == "table" then go(v) end
      end
   end
   go(result.ast)
   return out
end

-- Resolver for the `union-exhaustive` lint: the members of the union type at (y, x), as
-- a set of names, or `false` for a value that is typed and not a union. Nothing here
-- reconstructs the union from the `is` tests; the checker already knows it, and knows the
-- residual after a narrowing too.
local function union_resolver(result, filename)
   local ok, report = pcall(tl.get_types, result)
   if not ok or type(report) ~= "table" then return nil end
   local by_pos = report.by_pos and report.by_pos[filename]
   if not by_pos then return nil end
   local function deref(id, depth)
      local t = report.types[id]
      if t and t.ref and depth < 8 then return deref(t.ref, depth + 1) end
      return t
   end
   return function(y, x)
      local id = by_pos[y] and by_pos[y][x]
      if not id then return nil end
      local t = deref(id, 0)
      if not t then return nil end
      if type(t.types) ~= "table" or #t.types < 2 then return false end
      local names = {}
      for _, mid in ipairs(t.types) do
         local m = deref(mid, 0)
         if not m or not m.str then return false end
         -- Compared on the last segment: a member reads as `A` where the test may be
         -- written `defs.A`, and both name the same record.
         names[m.str:match("([^.]+)$") or m.str] = true
      end
      return names
   end
end

-- Resolvers for the enum boundary lints, built from one type report:
--
--   `cast_at(y, x, from_y, from_x)` — for the `as` expression at (y, x): the enum it casts
--   to (`{ name, values }`) and the checker's name for the type of the value being cast
--   (`"string"`, `"defs.State | nil"`, ...). nil when the cast does not land on an enum.
--
--   `enum_table_at(y, x)` — for the table constructor at (y, x): the enum its declared
--   type maps, in key or in value position, with the type as the checker writes it
--   (`{string : defs.State}`). nil when the declared type maps no enum, and when there is
--   no declared type at all — which is what exempts a table nobody annotated. An array of
--   an enum answers nil as well: it is a selection, not a mapping (see lint.lua).
--
-- Both answer from positions rather than from the tree, for the reason `struct_at` does:
-- lint.lua parses afresh and never touches the checker's annotated tree, so the type side
-- is answered here and handed over as a lookup.
local function enum_boundary_resolvers(result, filename)
   local ok, report = pcall(tl.get_types, result)
   if not ok or type(report) ~= "table" then return nil, nil end
   local by_pos = report.by_pos and report.by_pos[filename]
   if not by_pos then return nil, nil end
   local function deref(id, depth)
      if not id then return nil end
      local t = report.types[id]
      if t and t.ref and depth < 8 then return deref(t.ref, depth + 1) end
      return t
   end
   local function at(y, x)
      return deref(by_pos[y] and by_pos[y][x], 0)
   end
   -- Values sorted: a report has to be the same from one run to the next, and the fix
   -- inserts them in the order they are named.
   local function enum_of(t)
      if not t or type(t.enums) ~= "table" then return nil end
      local values = {}
      for _, v in ipairs(t.enums) do values[#values + 1] = v end
      table.sort(values)
      return { name = t.str or "enum", values = values }
   end
   local cast_at = function(y, x, from_y, from_x)
      local target = enum_of(at(y, x))
      if not target then return nil end
      local from = at(from_y, from_x)
      return target, from and from.str or nil
   end
   local enum_table_at = function(y, x)
      local t = at(y, x)
      if not t then return nil end
      local key, value = enum_of(deref(t.keys, 0)), enum_of(deref(t.values, 0))
      if not (key or value) then return nil end
      return { name = t.str or "table", key = key, value = value }
   end
   return cast_at, enum_table_at
end

-- Resolver for lints: type of a dotted subject (`c` / `w.state`) at (y, x).
-- Returns (enumset, type name) for an enum, `false` for a known non-enum type,
-- nil when unknown.
local function subject_enum_resolver(result, filename)
   local ok, report = pcall(tl.get_types, result)
   if not ok or type(report) ~= "table" then return nil end
   local function deref(id, depth)
      local t = report.types[id]
      if t and t.ref and depth < 8 then return deref(t.ref, depth + 1) end
      return t
   end
   return function(y, x, key)
      local syms = tl.symbols_in_scope(report, y, x, filename)
      local parts = {}
      for p in key:gmatch("[^.]+") do parts[#parts + 1] = p end
      local id = syms[parts[1]]
      if not id then return nil end
      local t = deref(id, 0)
      for i = 2, #parts do
         if not t or not t.fields then return nil end
         local fid = t.fields[parts[i]]
         if not fid then return nil end
         t = deref(fid, 0)
      end
      if not t then return nil end
      if t.enums then
         local set = {}
         for _, v in ipairs(t.enums) do set[v] = true end
         return set, t.str or "enum"
      end
      return false
   end
end

H.env = new_env()

local function fmt(filename, e)
   return string.format("%s:%d:%d: %s", e.filename or filename, e.y or 0, e.x or 0, e.msg or "?")
end

-- The warnings a check result carries, as text, each under the name of its kind.
--
-- Teal tags every warning it raises (`Errors:add_warning(tag, ...)` in the vendored
-- compiler) with one of seven kinds, and htl used to drop the tag on the floor here: every
-- Teal warning reached the user as an anonymous `warning:` line whose `rule` was null in
-- `--format json`. It is written as the trailing ` [htl tl:<kind>]` htl's own lints already
-- use, so one shape carries every name htl prints and `Diagnostic::parse` needs no second
-- case for these.
--
-- Naming them makes them addressable, and this is where that is answered, because this is
-- where they are collected: a kind the run turned off is dropped, and so is one whose line
-- carries `-- htl: allow(tl:<kind>)`. Both before the caller counts them, so `[lint] strict`
-- judges a run on what it said rather than on what it suppressed.
--
-- A kind the registry does not know is reported rather than dropped. htl vendors the
-- compiler, so a Teal upgrade that adds an eighth kind should carry it through under its
-- own name and be visible until htl catches up — not disappear because a table has no
-- entry for it.
local function warnings_of(filename, result, src)
   local out = {}
   local allows = {} -- file -> line -> allowed names, read at most once and only if asked
   local function allowed(file, y, rule)
      if not y or y == 0 then return false end
      local a = allows[file]
      if a == nil then
         -- `src` is the text of `filename` and of no other file: a caller that already
         -- read it (`H.gen_string`) hands it over so it is not read twice, and a warning
         -- pointing anywhere else is answered from disk.
         local text = (file == filename) and src or nil
         if not text then
            local fd = io.open(file, "rb")
            if fd then text = fd:read("a"); fd:close() end
         end
         a = text and lint.collect_allows(text) or false
         allows[file] = a
      end
      return a and a[y] and a[y][rule] == true
   end
   for _, w in ipairs(result.warnings or {}) do
      local rule = w.tag and ("tl:" .. w.tag)
      local file = w.filename or filename
      if not rule then
         out[#out + 1] = fmt(filename, w)
      elseif H.tl_cfg[rule] ~= false and not allowed(file, w.y, rule) then
         out[#out + 1] = fmt(filename, w) .. " [htl " .. rule .. "]"
      end
   end
   return out
end

local function norm_path(p)
   p = tostring(p):gsub("^%./", "")
   return p:lower()
end

-- On a case-insensitive filesystem `require("site")` from `Site.tl` finds the requiring
-- file itself; Teal then reports "no type information for required module" (or a
-- circular-require shape) with no hint why. Re-resolve the module and say so.
local function explain_self_require(filename, e)
   local msg = e.msg or ""
   local name = msg:match("no type information for required module: '([^']+)'")
      or msg:match("module not found: '([^']+)'")
      or msg:match("circular require: '([^']+)'")
   if not name then return msg end
   local found, fd = tl.search_module(name, true)
   if fd then fd:close() end
   if found and norm_path(found) == norm_path(filename) then
      return msg .. string.format(
         " (module '%s' resolved to '%s', the requiring file itself: the filesystem is case-insensitive " ..
         "and the module name collides with this file's name; rename one of them)", name, found)
   end
   return msg
end

-- Every `require("<literal>")` call site in the file, with where the checker resolves
-- it: { name, y, x, path } (path nil when unresolved). Feeds the self-require error and
-- the project-level require-cycle lint.
-- "wrong number of arguments (given N, expects M)" where the call's last argument is
-- itself a call: its multiple return values all expanded into arguments. Say so, with
-- the two idiomatic fixes; the bare count points at the outer call and mystifies.
local function callee_name(n)
   if type(n) ~= "table" then return nil end
   if n.kind == "variable" or n.kind == "identifier" then return n.tk end
   if n.kind == "op" and n.op and (n.op.op == "." or n.op.op == ":") then
      local a, b = callee_name(n.e1), callee_name(n.e2)
      if a and b then return a .. n.op.op .. b end
   end
   return nil
end

local function explain_arity(ast, e, msg)
   local given, expects = msg:match("^wrong number of arguments %(given (%d+), expects (%d+)%)")
   if not given or not ast then return msg end
   given, expects = tonumber(given), tonumber(expects)
   if given <= expects then return msg end
   local hit
   local seen = {}
   local function go(n)
      if hit or type(n) ~= "table" or seen[n] then return end
      seen[n] = true
      if n.kind == "op" and n.op and (n.op.op == "@funcall" or n.op.op == "@methcall")
         and n.y == e.y and n.x == e.x and type(n.e2) == "table" then
         local last = n.e2[#n.e2]
         if type(last) == "table" and last.kind == "op" and last.op
            and (last.op.op == "@funcall" or last.op.op == "@methcall") then
            hit = last
            return
         end
      end
      for k, v in pairs(n) do
         if k ~= "y" and k ~= "x" and type(v) == "table" then go(v) end
      end
   end
   go(ast)
   if not hit then return msg end
   local name = callee_name(hit.e1)
   local call = name and (name .. "(...)") or "the last argument"
   local extra = given - expects
   return msg .. string.format(
      ": %s is a call in last position, so all of its return values expand into arguments here (%d extra); " ..
      "bind them first (`local a, b = %s`) or wrap it in parentheses `(%s)` to keep only the first",
      call, extra, call, call)
end

-- The `(args): rets` part of the function header starting at source line `y`, as
-- written (headers may span lines; a trailing comment is dropped).
local function header_sig(src, y)
   local lines, i = {}, 0
   for line in (src .. "\n"):gmatch("([^\n]*)\n") do
      i = i + 1
      if i >= y then lines[#lines + 1] = line end
      if i >= y + 12 then break end
   end
   local text = table.concat(lines, "\n")
   local p = text:find("(", 1, true)
   if not p then return nil end
   local depth, q = 0, p
   while q <= #text do
      local ch = text:sub(q, q)
      if ch == "(" then
         depth = depth + 1
      elseif ch == ")" then
         depth = depth - 1
         if depth == 0 then break end
      end
      q = q + 1
   end
   if depth ~= 0 then return nil end
   local sig = text:sub(p, q)
   local rets = text:sub(q + 1):match("^[ \t]*(:[^\n]*)")
   if rets then
      rets = rets:gsub("%s*%-%-.*$", ""):gsub("%s+return%s.*$", ""):gsub("%s+end%s*$", "")
      sig = sig .. rets
   end
   return (sig:gsub("%s+", " "):gsub("%( ", "("):gsub(" %)", ")"))
end

-- "invalid key 'X' in record 'M'" where `function M.X(...)` is defined further down
-- the same file: Teal adds a record's fields in source order, so the use came too
-- early. Say so, and hand over the declaration line that makes the order irrelevant.
-- The `local record <rec>` declaration at the top level of the file, if any.
local function record_decl(ast, rec)
   for _, s in ipairs(ast) do
      if type(s) == "table" and (s.kind == "local_type" or s.kind == "global_type")
         and s.var and s.var.tk == rec and s.value and s.value.newtype then
         return s
      end
   end
   return nil
end

-- Where and how to insert `<key>: function<sig>` into the record: just before its
-- closing `end`, indented like the last field (or one indent deeper than the header
-- for an empty record). nil when the record's end line is unknown.
local function forward_ref_fix(src, decl, line)
   local lines, i = {}, 0
   for l in (src .. "\n"):gmatch("([^\n]*)\n") do
      i = i + 1
      lines[i] = l
   end
   local yend = decl.yend
   if not yend then
      -- No end position on the node: the record's `end` is the first line at the
      -- header's own indentation that is exactly `end`.
      local head_indent = (lines[decl.y] or ""):match("^(%s*)")
      for j = decl.y + 1, #lines do
         if lines[j]:match("^" .. head_indent .. "end%s*$") then
            yend = j
            break
         end
      end
   end
   if not yend or yend <= decl.y then return nil end
   local indent
   for j = yend - 1, decl.y + 1, -1 do
      local l = lines[j]
      if l and l:match("%S") then
         indent = l:match("^(%s*)")
         break
      end
   end
   if not indent then
      indent = (lines[decl.y] or ""):match("^(%s*)") .. "   "
   end
   return {
      applicability = "safe",
      edits = { { line = yend, col = 1, end_line = yend, end_col = 1, text = indent .. line .. "\n" } },
   }
end

-- Returns msg, fix (fix nil when the record's closing line cannot be located).
local function explain_forward_ref(ast, src, e, msg)
   local key, rec = msg:match("^invalid key '([%w_]+)' in record '([%w_]+)'")
   if not key or not ast or not src then return msg end
   for _, s in ipairs(ast) do
      if type(s) == "table" and s.kind == "record_function" and s.fn_owner and s.name
         and s.fn_owner.tk == rec and s.name.tk == key and s.y and s.y > (e.y or 0) then
         local sig = header_sig(src, s.y) or "(...)"
         if s.is_method then
            sig = sig:gsub("^%(%s*%)", "(self: " .. rec .. ")", 1):gsub("^%(", "(self: " .. rec .. ", ", 1)
         end
         local decl_line = key .. ": function" .. sig
         local explained = msg .. string.format(
            ": `%s.%s` is defined at line %d, after this use, and Teal adds a record's fields in " ..
            "source order. Declare it up front inside `record %s`: `%s` -- or move the " ..
            "definition above line %d",
            rec, key, s.y, rec, decl_line, e.y or 0)
         local decl = record_decl(ast, rec)
         local fix = decl and forward_ref_fix(src, decl, decl_line) or nil
         return explained, fix
      end
   end
   return msg
end

local function require_sites(ast)
   local out, seen = {}, {}
   local function go(n)
      if type(n) ~= "table" or seen[n] then return end
      seen[n] = true
      if type(n.kind) == "string" and n.kind == "op" and n.op and n.op.op == "@funcall"
         and type(n.e1) == "table" and n.e1.kind == "variable" and n.e1.tk == "require"
         and type(n.e2) == "table" and type(n.e2[1]) == "table" and n.e2[1].kind == "string" then
         local tk = n.e2[1].tk or ""
         local name = tk:sub(2, -2)
         local found, fd = tl.search_module(name, true)
         if fd then fd:close() end
         out[#out + 1] = { name = name, y = n.y, x = n.x, path = found }
      end
      for k, v in pairs(n) do
         if k ~= "if_parent" and k ~= "type" and k ~= "newtype" and k ~= "decltuple" and k ~= "expected"
            and type(v) == "table" then go(v) end
      end
   end
   go(ast)
   return out
end

-- Proactive form of the same check: every `require("<literal>")` in the file whose
-- resolution is the file itself gets its own error at the call site. Teal may swallow
-- the self-require as a circular require and only complain later ("unknown type
-- site.Config"), which hides the cause.
local function self_require_errors(filename, ast)
   local out = {}
   for _, r in ipairs(require_sites(ast)) do
      if r.path and norm_path(r.path) == norm_path(filename) then
         out[#out + 1] = {
            y = r.y, x = r.x,
            msg = string.format(
               "require(\"%s\") resolves to '%s', the requiring file itself: the filesystem is " ..
               "case-insensitive and the module name collides with this file's name; rename one of them",
               r.name, r.path),
         }
      end
   end
   return out
end

local function collect_errors(filename, result, src)
   -- A result served again from the env cache (every runtime `require` of a module
   -- already checked) would otherwise re-walk its AST for require sites and re-resolve
   -- each one on disk: ~11 ms per module, ~1.4 s over a 261-test run [measured].
   if result.htl_errors and result.htl_errors_for == filename then
      return result.htl_errors, result.htl_error_fixes
   end
   local errors = {}
   -- error_fixes[i] = fix for errors[i], or false: a rewrite `htl fix` may apply.
   local error_fixes = {}
   result.htl_errors, result.htl_error_fixes, result.htl_errors_for = errors, error_fixes, filename
   for _, e in ipairs(result.syntax_errors or {}) do errors[#errors + 1] = fmt(filename, e) end
   if result.ast and #(result.syntax_errors or {}) == 0 then
      -- Cheap text prefilter: only when some `require("<name>")` in the source resolves
      -- to this very file is the AST walked for exact positions. The walk costs tens of
      -- ms on a large module and it ran for every module a program required [measured].
      if not src then
         local fd = io.open(filename, "rb")
         if fd then src = fd:read("a"); fd:close() end
      end
      local suspicious = false
      for name in (src or ""):gmatch("require%s*%(?%s*[\"']([^\"']+)[\"']") do
         local found, fd = tl.search_module(name, true)
         if fd then fd:close() end
         if found and norm_path(found) == norm_path(filename) then suspicious = true break end
      end
      if suspicious then
         for _, e in ipairs(self_require_errors(filename, result.ast)) do errors[#errors + 1] = fmt(filename, e) end
      end
   end
   local hinted = {} -- lines where an arity error was explained by a multi-value call
   -- Keys an `---@extensible` record allows, resolved on the first `unknown field` error
   -- and not before: the walk and the type report cost something, and a file with no such
   -- error has nothing for them to answer.
   local extensible
   local function extensible_allows(e, key)
      if extensible == nil then extensible = extensible_keys(filename, result) end
      return extensible[(e.y or 0) .. ":" .. (e.x or 0)] == key
   end
   for _, e in ipairs(result.type_errors or {}) do
      local msg = explain_self_require(filename, e)
      local own = e.filename == nil or e.filename == filename
      local fix
      if own then
         local explained = explain_arity(result.ast, e, msg)
         if explained ~= msg then hinted[e.y] = true end
         msg, fix = explain_forward_ref(result.ast, src, e, explained)
      end
      -- tl follows the arity error with "argument N: got X, expected T (unresolved
      -- generic)" for the very same call: a consequence, not a second mistake.
      local dropped = own and hinted[e.y] and msg:find("(unresolved generic)", 1, true)
      -- `unknown field extra` about a key an `---@extensible` record does not declare is
      -- the marker working: the record said its declaration is not the whole set.
      if not dropped then
         local key = own and msg:match("unknown field ([%w_]+)$")
         dropped = key and extensible_allows(e, key)
      end
      if not dropped then
         errors[#errors + 1] = fmt(filename, { filename = e.filename, y = e.y, x = e.x, msg = msg })
         error_fixes[#errors] = fix or false
      end
   end
   -- syntax / self-require errors carry no fix
   for i = 1, #errors do
      if error_fixes[i] == nil then error_fixes[i] = false end
   end
   return errors, error_fixes
end

-- Collect every enum reachable from a tl type object (records nest enums via
-- `.fields`, typedecls wrap via `.def`). `out[name] = enumset`.
local function collect_type_enums(t, path, out, seen, depth)
   if type(t) ~= "table" or seen[t] or depth > 12 then return end
   seen[t] = true
   if t.typename == "enum" and t.enumset then
      out[path] = t.enumset
   end
   if t.def then collect_type_enums(t.def, path, out, seen, depth + 1) end
   if t.fields then
      for k, v in pairs(t.fields) do
         collect_type_enums(v, path .. "." .. tostring(k), out, seen, depth + 1)
      end
   end
end

-- Enums the checker knows for one checked file: the file's own types (nested included)
-- and every module it required (so `defs.Behavior` counts for enum-exhaustive).
local function checked_enums(result, env)
   local out, seen = {}, {}
   for _, node in ipairs(result.ast or {}) do
      if (node.kind == "local_type" or node.kind == "global_type") and node.value and node.value.newtype then
         collect_type_enums(node.value.newtype, node.var and node.var.tk or "?", out, seen, 0)
      end
   end
   if result.type then collect_type_enums(result.type, "<module>", out, seen, 0) end
   for name, mod in pairs(env.modules or {}) do
      collect_type_enums(mod, name, out, seen, 0)
   end
   return out
end

-- Type-check one file. Returns { ok, errors = {string}, warnings = {string}, result = tl Result }
-- Uses a fresh env so module names resolved for one file (via its package.path)
-- never leak into a later file from another directory. `H.gen` keeps the shared
-- env: it serves one program (run / build / include_tl!) where sharing is wanted.
local PROFILE = os.getenv("HTL_PROFILE") ~= nil
local function prof(label, filename, t0)
   if PROFILE then
      io.stderr:write(string.format("profile: %-8s %7.1f ms  %s\n", label, (os.clock() - t0) * 1000, filename))
   end
end

-- Checked-module store, shared by every fresh env in this state. A fresh env per file
-- exists so that module *names* resolve under that file's own search path and never
-- leak from another directory; the store keeps that guarantee by seeding an env only
-- with entries whose name still resolves to the very same file here. What is shared is
-- the result of checking a file, which does not depend on who required it.
local store = {} -- module name -> { filename, type, result }

local function store_from(env)
   for name, ty in pairs(env.modules) do
      local fname = env.module_filenames[name]
      local result = fname and env.loaded[fname]
      -- skip the placeholder tl leaves while a module is being checked (circular requires)
      if result and result.type == ty then
         store[name] = { filename = fname, type = ty, result = result }
      end
   end
end

local function seed_env(env)
   for name, e in pairs(store) do
      if env.modules[name] == nil then
         local found, fd = tl.search_module(name, true)
         if fd then fd:close() end
         if found == e.filename then
            env.modules[name] = e.type
            env.module_filenames[name] = e.filename
            env.loaded[e.filename] = e.result
         end
      end
   end
end

function H.reset_store()
   store = {}
end

-- The type errors of every module a check pulled in through `require`, transitively.
--
-- tl checks a required module into the same env (`env.loaded[file]` holds its full
-- result, errors included) and hands the requirer only its *type*, so a broken
-- dependency leaves no trace in the requirer's own error list — `htl check` used to
-- pass a project whose first `require` at run time would raise. Each dependency is
-- listed once per walk, against the file that first required it; the caller decides
-- what to do about a dependency it has already seen from another file.
--
-- `result.dependencies` is name -> file for the direct requires; a dependency that
-- was seeded from the store into this env may not have had its own requires loaded
-- here, so the store is asked for those. Names are visited sorted: the map has no
-- order of its own, and a report has to be the same from one run to the next.
local function dependency_errors(filename, result, env)
   local out = {}
   local seen = { [filename] = true }
   local function walk(res, requirer)
      local names = {}
      for name in pairs(res.dependencies or {}) do names[#names + 1] = name end
      table.sort(names)
      for _, name in ipairs(names) do
         local fname = res.dependencies[name]
         if not seen[fname] then
            seen[fname] = true
            local dep = env.loaded and env.loaded[fname]
            if not dep then
               local e = store[name]
               if e and e.filename == fname then dep = e.result end
            end
            if dep then
               local errs = collect_errors(fname, dep)
               for _, text in ipairs(errs) do
                  out[#out + 1] = { file = fname, required_by = requirer, text = text }
               end
               walk(dep, fname)
            end
         end
      end
   end
   walk(result, filename)
   return out
end

-- opts.lints = false skips the lint pass (runtime `require` of an already type-checked
-- module: nobody reads lints there, and the pass costs more than the check itself).
-- opts.seed = false checks with a cold env (no store): what is on disk right now, rather
-- than what the store remembers. A caller that just wrote the file has to ask this way —
-- tl.check_file returns early when the env already has the file loaded, which a seed puts
-- there, so a re-check would answer about the version before the write.
-- opts.store = false leaves the store untouched, for a check whose result may be about a
-- file that is then reverted: storing it would leave the store describing a file that no
-- longer says that.
function H.check(filename, env, opts)
   opts = opts or {}
   env = env or new_env() -- bind first: assert() would also pass its message along as `fd`
   local t0 = os.clock()
   -- Seed on first use of an env (not at creation): by now the caller has set up the
   -- search path this program resolves through, which is what the seed validates against.
   if not env.htl_seeded then
      env.htl_seeded = true
      if opts.seed ~= false then seed_env(env) end
   end
   local result, err = tl.check_file(filename, env)
   prof("check", filename, t0)
   if result and opts.store ~= false then store_from(env) end
   if not result then
      return { ok = false, errors = { tostring(err) }, warnings = {} }
   end
   t0 = os.clock()
   local errors, error_fixes = collect_errors(filename, result)
   local warnings = warnings_of(filename, result)
   local deps = {}
   for _, fname in pairs(result.dependencies or {}) do deps[#deps + 1] = fname end
   table.sort(deps)
   local lints, lint_fixes = {}, {}
   if opts.lints ~= false and result.ast and #(result.syntax_errors or {}) == 0 then
      local src
      local fd = io.open(filename, "rb")
      if fd then src = fd:read("a"); fd:close() end
      if src then
         local t1 = os.clock()
         local enums = checked_enums(result, env)
         prof("enums", filename, t1)
         t1 = os.clock()
         local subject = subject_enum_resolver(result, filename)
         prof("get_types", filename, t1)
         t1 = os.clock()
         local cast_at, enum_table_at = enum_boundary_resolvers(result, filename)
         local found = lint.run(src, filename, H.lint_cfg, {
            enums = enums,
            subject_enum = subject,
            struct_at = struct_resolver(result, filename),
            sealed_at = sealed_resolver(result, filename),
            nilable_at = nilable_resolver(result, filename),
            deps = H.deps,
            union_at = union_resolver(result, filename),
            cast_at = cast_at,
            enum_table_at = enum_table_at,
         })
         prof("lint.run", filename, t1)
         for _, l in ipairs(found or {}) do
            lints[#lints + 1] = fmt(filename, l)
            lint_fixes[#lints] = l.fix or false
         end
      end
   end
   local requires = {}
   -- require sites feed the project-level require-cycle lint: same gate as the lints.
   if opts.lints ~= false and result.ast then requires = require_sites(result.ast) end
   prof("lint+req", filename, t0)
   -- Errors in what this file required. `ok` stays the file's own answer: `H.gen` still
   -- generates it, and the searcher refuses the dependency itself on its first `require`.
   local dep_errors = dependency_errors(filename, result, env)
   return { ok = #errors == 0, errors = errors, error_fixes = error_fixes, warnings = warnings, deps = deps,
      lints = lints, lint_fixes = lint_fixes, requires = requires, dependency_errors = dep_errors, result = result }
end

-- Type-check + generate Lua source. Returns code, checkinfo (code is nil on failure).
-- Type-check + generate Lua for one program. Uses the shared env on purpose: a module
-- already checked while checking its requirer (or an earlier `require`) is served from
-- `env.modules` instead of being checked again with all of its dependencies. Measured
-- on a 7k-line project: a test file went from 5.0 s to the cost of one `htl check`.
function H.gen(filename, opts)
   local c = H.check(filename, H.env, opts)
   if not c.ok then
      return nil, c
   end
   -- Generated once per checked result: the result object is what the env cache (and
   -- the store behind it) hands back, so the code rides along with it.
   if c.result.htl_code then
      return c.result.htl_code, c
   end
   local t0 = os.clock()
   local code, gerr = tl.generate(c.result.ast, H.GEN_TARGET)
   prof("generate", filename, t0)
   if code then c.result.htl_code = code end
   if not code then
      c.ok = false
      c.errors = { filename .. ": generate failed: " .. tostring(gerr) }
      return nil, c
   end
   return code, c
end

-- Type-check + generate from source text (used by the mlua-pkg resolver, where the
-- sandbox already read the file). Same return shape as H.gen.
function H.gen_string(src, filename)
   local result = tl.check_string(src, H.env, filename)
   local errors = collect_errors(filename, result, src)
   local warnings = warnings_of(filename, result, src)
   local c = { ok = #errors == 0, errors = errors, warnings = warnings, deps = {}, lints = {}, result = result }
   if not c.ok or not result.ast then
      return nil, c
   end
   local code, gerr = tl.generate(result.ast, H.GEN_TARGET)
   if not code then
      c.ok = false
      c.errors = { filename .. ": generate failed: " .. tostring(gerr) }
      return nil, c
   end
   return code, c
end

-- Value handed to `require` for a declaration-only module (`name.d.tl` with no
-- implementation on the path). Indexing it explains what is missing instead of the
-- bare "attempt to call a nil value" that would surface otherwise.
function H.type_only_module(module_name, decl_path)
   return setmetatable({}, {
      __index = function(_, key)
         error(string.format(
            "module '%s' is declaration-only here (%s): '%s' has no implementation on this path. " ..
            "It must be provided by the host program (e.g. a Rust #[host_module] via cargo run) " ..
            "or by a .tl/.lua module with that name.",
            module_name, decl_path, tostring(key)), 2)
      end,
   })
end

-- Declared field names of a record type reachable as `<module>.<Type>`. The declaring
-- module is loaded into the shared env on first use (its declarations are the same for
-- every contract dir, unlike the modules held to them, so sharing is right here).
-- Returns a sorted list, or nil when the type cannot be found.
function H.record_fields(type_path)
   local module, tname = type_path:match("^([^.]+)%.(.+)$")
   if not module then return nil end
   local mod = H.env.modules and H.env.modules[module]
   if not mod then
      tl.check_string(string.format('local m = require("%s")\nreturn m\n', module), H.env,
         "<record_fields " .. module .. ">")
      mod = H.env.modules and H.env.modules[module]
   end
   if not mod then return nil end
   local t = mod
   for seg in tname:gmatch("[^.]+") do
      if t.def then t = t.def end
      if not (t.fields and t.fields[seg]) then return nil end
      t = t.fields[seg]
   end
   if t.def then t = t.def end
   if not t.fields then return nil end
   local names = {}
   for k in pairs(t.fields) do names[#names + 1] = k end
   table.sort(names)
   return names
end

-- Static contract check for one module file (the `contract` lint):
--   1. `local m: <type_path> = require("<modname>")` through the checker (type errors),
--   2. with require_fields (`true` for every declared field, or a list of names): keys
--      of the module's returned table literal (also `X.define({ ... })`) vs those.
-- Returns { errors = {string}, missing = {string} | nil (nil = not decidable),
--           bad_require_fields = {string} (names the type does not declare) }.
-- Type-check a stub in a fresh env: the shared env caches module types by name, so a
-- second `Site` (another contract dir) would be judged by the first one's type.
-- Literal `require`s of a plain Lua file (a vendored dependency), resolved like the
-- checker resolves them. Parsed with tl in Lua mode; a file tl cannot parse yields
-- no sites (its requires are then the host's to declare).
function H.lua_requires(src, filename)
   local ast, errs = tl.parse(src, filename, "lua")
   if not ast or (errs and #errs > 0) then return {} end
   return require_sites(ast)
end

-- Where `require(name)` would resolve for the checker (`.tl` / `.d.tl` / `.lua`), and
-- where a plain `.lua` implementation sits on the path, if any. Both may be nil.
function H.resolve_module(name)
   local found, fd = tl.search_module(name, true)
   if fd then fd:close() end
   local lua_path = package.searchpath(name, package.path)
   return found, lua_path
end

-- Every file on the current `package.path` that could answer `require(name)`, in the
-- order the searchers consult them: sources across the whole path, then declarations,
-- then plain Lua — the order the wrapper at the top of this file gives `tl.search_module`,
-- which is why source beats declaration wherever the two sit.
--
-- The searchers answer with the first hit and say nothing about the rest, which is the
-- whole question: one file is read, the others are not, decided by a position nobody
-- wrote down. Each entry is { path, kind = "source" | "declaration" | "lua", dir },
-- `dir` being the search-path directory the template it was found through belongs to.
function H.module_candidates(name)
   local out, seen = {}, {}
   local relative = (name:gsub("%.", "/"))
   for _, ext in ipairs({ { ".tl", "source" }, { ".d.tl", "declaration" }, { ".lua", "lua" } }) do
      for template in package.path:gmatch("[^;]+") do
         -- The path templates end in `.lua` (the searchers rewrite the suffix); anything
         -- else on the path is not ours to interpret.
         if template:sub(-4) == ".lua" then
            local p = (template:sub(1, -5) .. ext[1]):gsub("%?", relative)
            if not seen[p] then
               seen[p] = true
               local fd = io.open(p, "r")
               if fd then
                  fd:close()
                  out[#out + 1] = { path = p, kind = ext[2], dir = H.template_dir(template) }
               end
            end
         end
      end
   end
   return out
end

-- The directory a `package.path` template searches: everything before its first `?`,
-- without the separator. `?.lua` (the cwd) is ".".
function H.template_dir(template)
   local head = template:match("^([^?]*)") or ""
   head = head:gsub("/+$", "")
   if head == "" then return "." end
   return head
end

-- The directories `package.path` searches, in order, one entry each however many
-- templates a directory contributes (`add_path` adds three).
function H.search_dirs()
   local out, seen = {}, {}
   for template in package.path:gmatch("[^;]+") do
      local dir = H.template_dir(template)
      if not seen[dir] then
         seen[dir] = true
         out[#out + 1] = dir
      end
   end
   return out
end

-- Every `<name>.d.tl` reachable on the current `package.path`, in the order the path is
-- consulted: the declarations of `module_candidates`, which is the same walk.
function H.declaration_sites(name)
   local out = {}
   for _, c in ipairs(H.module_candidates(name)) do
      if c.kind == "declaration" then
         out[#out + 1] = c.path
      end
   end
   return out
end

-- Statements of a file as { first_line, last_line } ranges, for coverage. Every
-- statement in every block (function bodies inside expressions included); an `if`
-- chain contributes each condition's line as its own range. Type declarations are
-- not statements that run. A range ends where the next statement in its block
-- starts, or at the node's own end when it is the last one.
local EXEC_KINDS = {
   local_declaration = true, assignment = true, ["return"] = true, ["if"] = true,
   ["while"] = true, ["repeat"] = true, forin = true, fornum = true, ["goto"] = true,
   ["break"] = true, ["do"] = true, local_function = true, global_function = true,
   record_function = true, op = true,
}

-- The named functions of a file, alongside the ranges: { name, y, last }, where the
-- body is what lies strictly between the two. Both ends are left out because defining
-- a function runs both of them: for a function nothing ever calls, the hook still
-- reports its `function` line (the closure is built there) and its `end` line (the
-- result is stored there). Measured on Lua 5.4: a never-called `function m.f()` at
-- 12..15 comes back as lines 12 and 15 hit, 13 and 14 not. A function with nothing
-- between its two lines therefore has no body to judge, and is left out.
local FN_KINDS = { local_function = true, global_function = true, record_function = true }

local function owner_name(n)
   if type(n) ~= "table" then return nil end
   if n.tk then return n.tk end
   if n.kind == "op" and n.op and n.op.op == "." then
      local a, b = owner_name(n.e1), owner_name(n.e2)
      if a and b then return a .. "." .. b end
   end
   return nil
end

-- `f`, `M.f`, `M:f` -- as the source writes it, so the report names something the
-- reader can search for.
local function function_name(n)
   local base = n.name and n.name.tk
   if not base then return nil end
   if n.kind ~= "record_function" then return base end
   local owner = owner_name(n.fn_owner)
   if not owner then return base end
   return owner .. (n.is_method and ":" or ".") .. base
end

function H.executable_ranges(filename)
   local fd = io.open(filename, "rb")
   if not fd then return nil end
   local src = fd:read("a")
   fd:close()
   local ast, errs = tl.parse(src, filename, "tl")
   if not ast or (errs and #errs > 0) then return nil end
   local ranges = {}
   local funcs = {}
   local seen = {}
   local function go(n)
      if type(n) ~= "table" or seen[n] then return end
      seen[n] = true
      if FN_KINDS[n.kind] and n.y then
         local last = n.yend or n.y
         local name = function_name(n)
         if name and last > n.y + 1 then
            funcs[#funcs + 1] = { name = name, y = n.y, last = last }
         end
      end
      if n.kind == "statements" then
         for i, s in ipairs(n) do
            if type(s) == "table" and s.kind and EXEC_KINDS[s.kind] and s.y then
               local nxt = n[i + 1]
               local last = (type(nxt) == "table" and nxt.y and nxt.y - 1) or s.yend or s.y
               if last < s.y then last = s.y end
               ranges[#ranges + 1] = { s.y, last }
               if s.kind == "if" and s.if_blocks then
                  for bi = 2, #s.if_blocks do
                     local b = s.if_blocks[bi]
                     if b.exp and b.y then ranges[#ranges + 1] = { b.y, b.y } end
                  end
               end
            end
         end
      end
      for k, v in pairs(n) do
         if k ~= "if_parent" and k ~= "type" and k ~= "newtype" and k ~= "decltuple" and k ~= "expected"
            and type(v) == "table" then go(v) end
      end
   end
   go(ast)
   table.sort(ranges, function(a, b) return a[1] < b[1] end)
   table.sort(funcs, function(a, b) return a.y < b.y end)
   return ranges, funcs
end

function H.check_stub(src, filename)
   local result = tl.check_string(src, new_env(), filename)
   return collect_errors(filename, result, src)
end

function H.contract_check(filename, modname, type_path, require_fields)
   local module = type_path:match("^([^.]+)%.")
   local out = { errors = {}, missing = nil }
   if not module then
      out.errors[1] = "contract type must be written as <module>.<Type>: " .. tostring(type_path)
      return out
   end
   local stub = string.format('local %s = require("%s")\nlocal m: %s = require("%s")\nreturn m\n',
      module, module, type_path, modname)
   out.errors = H.check_stub(stub, "<contract " .. type_path .. " for " .. modname .. ">")
   if #out.errors > 0 then return out end
   if not require_fields then return out end
   local declared = H.record_fields(type_path)
   if not declared then return out end
   -- `require_fields` is either true (everything the type declares) or the list from
   -- htl.toml. A name in the list the type does not declare is a mistake in the config
   -- and is said as one: silently ignoring it would make the contract quietly weaker
   -- than it reads.
   local wanted = declared
   if type(require_fields) == "table" then
      local is_declared = {}
      for _, f in ipairs(declared) do is_declared[f] = true end
      local unknown = {}
      for _, f in ipairs(require_fields) do
         if not is_declared[f] then unknown[#unknown + 1] = f end
      end
      if #unknown > 0 then
         table.sort(unknown)
         -- Its own field, not an entry in `errors`: this is the config being wrong about
         -- the type, not a module failing to satisfy it, and the two read differently.
         out.bad_require_fields = unknown
         return out
      end
      wanted = require_fields
   end
   -- Keys of the returned table literal, if the module ends in one.
   local fd = io.open(filename, "rb")
   if not fd then return out end
   local src = fd:read("a")
   fd:close()
   local ast = tl.parse(src, filename, "tl")
   if not ast then return out end
   local ret, ret_i
   for i = #ast, 1, -1 do
      local s = ast[i]
      if type(s) == "table" and s.kind == "return" then ret, ret_i = s, i break end
   end
   if not ret or not ret.exps or not ret.exps[1] then return out end
   local exp = ret.exps[1]
   local present = {}
   local function strip_cast(e)   -- `{ ... } as T`
      if e and e.kind == "op" and e.op and e.op.op == "as" then return e.e1 end
      return e
   end
   exp = strip_cast(exp)
   -- `return m`: find the table literal `m` was declared / assigned from, and count
   -- `m.<field> = ...` statements in between as present.
   if exp.kind == "variable" then
      local name = exp.tk
      local found
      for i = ret_i - 1, 1, -1 do
         local s = ast[i]
         if type(s) == "table" and s.vars then
            for vi, v in ipairs(s.vars) do
               -- declared names parse as `identifier`, assigned ones as `variable`
               if (s.kind == "local_declaration" or s.kind == "assignment")
                  and (v.kind == "variable" or v.kind == "identifier") and v.tk == name
                  and s.exps and s.exps[vi] then
                  found = s.exps[vi]
               elseif s.kind == "assignment" and v.kind == "op" and v.op and v.op.op == "."
                  and v.e1 and v.e1.kind == "variable" and v.e1.tk == name and v.e2 and v.e2.tk then
                  present[v.e2.tk] = true
               end
            end
         end
         if found then break end
      end
      if not found then return out end
      exp = strip_cast(found)
   end
   -- `return X.define({ ... })` / `return define({ ... })`: look at the single literal argument
   if exp.kind == "op" and exp.op and exp.op.op == "@funcall" and exp.e2 and exp.e2[1]
      and exp.e2[1].kind == "literal_table" and #exp.e2 == 1 then
      exp = exp.e2[1]
   end
   exp = strip_cast(exp)
   if exp.kind ~= "literal_table" then return out end
   for _, item in ipairs(exp) do
      if type(item) == "table" and item.key and item.key.kind == "string" then
         present[(item.key.tk or ""):sub(2, -2)] = true
      elseif type(item) == "table" and item.key and item.key.kind == "identifier" then
         present[item.key.tk] = true
      end
   end
   out.missing = {}
   for _, f in ipairs(wanted) do
      if not present[f] then out.missing[#out.missing + 1] = f end
   end
   out.missing_y, out.missing_x = ret.y, ret.x
   return out
end

-- What a runtime `require` should do for `module_name`, as data (so a runtime state
-- living elsewhere can ask the same question, see Htl::with_checker):
--   "code", code, found   generated Lua for a type-checked .tl
--   "type_only", dfound   declaration-only module (`name.d.tl`, no .lua behind it):
--                         hand require a table that explains itself on first use
--   "yield", msg          a .lua sibling exists: step aside for Lua's own searcher
--   "missing", msg        nothing on package.path
-- Type errors raise: unlike tl.loader(), they are fatal at require time.
local function resolve_for_require(module_name)
   local found, fd = tl.search_module(module_name, false)
   if not found then
      local dfound, dfd = tl.search_module(module_name, true)
      if dfound and dfound:match("%.d%.tl$") then
         dfd:close()
         -- A `.lua` implementation anywhere on the path (a vendored dependency next to
         -- its declaration, or declared from `src/` and implemented elsewhere) is served
         -- by Lua's own searcher; the declaration only typed it.
         local lua_path = package.searchpath(module_name, package.path)
         if lua_path then
            return "yield", "\n\ttype-only '" .. dfound .. "' (implementation served by the .lua searcher)"
         end
         return "type_only", dfound
      elseif dfd then
         dfd:close()
      end
      return "missing", "\n\tno .tl module '" .. module_name .. "' on package.path"
   end
   fd:close()
   -- Lints are the CLI's business (`htl check`), not require's.
   local code, c = H.gen(found, { lints = false })
   if not code then
      error(table.concat(c.errors, "\n"), 0)
   end
   return "code", code, found
end

H.gen_for_require = resolve_for_require

-- Strict searcher for a state that hosts its own checker.
local function strict_searcher(module_name)
   local kind, a, b = resolve_for_require(module_name)
   if kind == "code" then
      local chunk, lerr = load(a, "@" .. b, "t")
      if not chunk then
         error("htl: generated Lua failed to load: " .. tostring(lerr), 0)
      end
      return function(modname)
         return chunk(modname, b)
      end, b
   elseif kind == "type_only" then
      return function() return H.type_only_module(module_name, a) end, a
   end
   return a
end

function H.install_searcher()
   table.insert(package.searchers, 2, strict_searcher)
end

function H.get_path()
   return package.path
end

function H.set_path(p)
   package.path = p
end

-- Start serving a new program (one per test file when the checker is shared): fresh
-- module-name resolution for it, seeded from the store so nothing is checked twice.
function H.begin_program()
   H.env = new_env() -- seeded on its first check, once the program's paths are set
end

-- tl.search_module rewrites the ".lua" suffix of each package.path template to
-- ".tl" / ".d.tl" / ".lua" in turn, so templates must end in ".lua".
-- `?/?.lua` lets a flat package expose its top-level module as `<name>/<name>.tl`
-- (mlua-pkg's entry is a directory; without init.tl this is how a flat layout resolves).
function H.add_path(dir)
   local templates = dir .. "/?.lua;" .. dir .. "/?/init.lua;" .. dir .. "/?/?.lua"
   if package.path == nil or package.path == "" then
      package.path = templates
   else
      package.path = templates .. ";" .. package.path
   end
end

-- Drop Lua's default search path (`./?.lua` etc., i.e. cwd-relative resolution) so only
-- directories given to add_path are consulted. Used by the proc macros, where the cwd
-- is cargo's and has nothing to do with the script being embedded.
function H.reset_path()
   package.path = ""
end

return H
