-- htl prelude: thin Lua-side layer over tl.lua (Teal compiler).
-- Loaded into every htl Lua state after `tl` is registered in package.preload.

local tl = require("tl")
local lint = require("htl.lint")
local fmt_mod = require("htl.fmt")
local H = {}

-- The requiring file, for the project model's resolver (`H.resolve_name`, installed by
-- `Htl::install_resolver`): the file htl's own walk over an AST names while it asks
-- (`H.asking`), else the innermost file Teal is checking (`H.requirers`, kept by the
-- `tl.check_string` wrapper below). Neither is set for a run-time `require`, which carries
-- only the name.
H.requirers = {}
function H.current_requirer()
   return H.asking or H.requirers[#H.requirers]
end

-- The environment the innermost check is running in, kept beside `H.requirers` by the
-- same wrapper. The `tl.search_module` wrapper below needs it: a `require` is answered
-- into an env, and what the store has to hand that env (a global-declaring module's
-- globals, see `deliver_globals`) goes into that env and no other. Empty outside a check,
-- which is how a search made for its own sake (`seed_env`'s) is told apart from one Teal
-- makes on behalf of a `require`.
H.envs = {}

-- filename -> store entry, every entry. Filled by `store_from` (further down, with the
-- store itself) and read here: a search answers with a path, and the path is what says
-- whether the file being required is one the store holds for this env; and an entry's
-- `result.dependencies` is name -> file, which this index turns back into entries.
local by_file = {}

-- Every global a `require` of the entry's module brings into scope: the names its own top
-- level declares (`globals`), and those of every module it requires, transitively. The
-- second half is what a walk would have done — walking `modx` runs its `require("host")`,
-- which registers `host`'s globals — and a result served from the store is not a walk, so
-- the requirer of a served module is handed what the walk would have registered below it.
-- Name -> var table, the first declaration found winning, the way the checker keeps the
-- first of two same-typed declarations.
--
-- Computed on first use and kept on the entry. That is never stale: `store_from` stores
-- an env whole, so every module walked in the env that walked this one was stored in the
-- same call, and a module served to that env from the store was stored earlier. The
-- entry is rebuilt by the next `store_from` that sees the module, which drops the memo —
-- the result object is the same, so what is recomputed is the same too. A cycle in the
-- requires (`require-cycle` is a lint, not an error) is cut at the entry already on the
-- path.
local function globals_below(e, visited)
   if e.closure then return e.closure end
   visited = visited or {}
   visited[e.filename] = true
   local out = {}
   for name, var in pairs(e.globals or {}) do out[name] = var end
   local deps = e.result.dependencies or {}
   local names = {}
   for name in pairs(deps) do names[#names + 1] = name end
   table.sort(names) -- the map has no order; the first declaration to win must be the same each run
   for _, name in ipairs(names) do
      local d = by_file[deps[name]]
      if d and not visited[d.filename] then
         for gname, var in pairs(globals_below(d, visited)) do
            if out[gname] == nil then out[gname] = var end
         end
      end
   end
   e.closure = out
   return out
end

-- Where every global a `require` of the entry's module brings into scope was declared:
-- `{ name, file, y, x }` per declaration, the entry's own (`sites`) and those of everything
-- below it, each site once. The same walk as `globals_below`, kept apart because it
-- answers a different question -- not "which var table is this name" but "who else
-- declared it": the `global-redeclaration` lint reads the sites off every check of a run
-- and reports a name with two. Sites rather than the walked files' own declarations
-- because a `.d.tl` is never a walked file of a directory check, and a run whose files all
-- replay from the cache walks nothing at all; the closure on the check is what survives
-- both. Memoised as `globals_below`'s closure is, for the same reason it is never stale.
local function sites_below(e, visited, out, seen)
   if e.closure_sites and not out then return e.closure_sites end
   local top = out == nil
   visited = visited or {}
   out = out or {}
   seen = seen or {}
   visited[e.filename] = true
   for _, s in ipairs(e.sites or {}) do
      local key = s.file .. ":" .. s.y .. ":" .. s.x
      if not seen[key] then
         seen[key] = true
         out[#out + 1] = s
      end
   end
   local deps = e.result.dependencies or {}
   local names = {}
   for name in pairs(deps) do names[#names + 1] = name end
   table.sort(names)
   for _, name in ipairs(names) do
      local d = by_file[deps[name]]
      if d and not visited[d.filename] then sites_below(d, visited, out, seen) end
   end
   if top then e.closure_sites = out end
   return out
end

-- Hand an env the globals of a module it is requiring and will not walk. The store seeds
-- such a module's checked result into `env.loaded` (`seed_env`), so `tl.check_file` will
-- return that result without reading the file — and a result replayed is not a walk, so
-- nothing would register into this env the `global`s the module declares, nor those the
-- modules it requires declare. The walks that did register them, once each, left their
-- var tables on the entries (`globals_below` collects them); putting the same tables into
-- this env's `env.globals` is what makes the names known here, and known as the same type
-- instances those walks produced. A name this env already has is left alone: the
-- checker's own rule for a second declaration (`add_global` in vendor/tl.lua: same type is
-- kept, a different one is an error) applies when the env walks a declaration of its own.
--
-- Only an env that was seeded with the entry's result is handed the globals: in one that
-- was not (the entry's name resolves to another file here, or seeding was off) the
-- `require` walks the file, and the walk registers what it and its requires declare.
local function deliver_globals(found)
   local env = H.envs[#H.envs]
   local e = env and by_file[found]
   if not (e and env.loaded[found] == e.result) then return false end
   for name, var in pairs(globals_below(e)) do
      if env.globals[name] == nil then env.globals[name] = var end
   end
   return true
end

-- Source beats declaration. tl's own search order is `.d.tl` across the whole path
-- first, then `.tl`, so a stale `mods/defs.d.tl` written by a host would shadow the
-- `src/defs.tl` it was made from wherever the two sit on the path. htl's run-time
-- searchers already try `.tl` before `.d.tl`; make the checker agree, so a declaration
-- is what you check against only when no source of that module is reachable.
-- (`require_module` looks `tl.search_module` up on each call, so wrapping it works.)
do
   local tl_search = tl.search_module
   local function search(module_name, search_all)
      -- A name the project model has is answered by the model and nothing else: its
      -- implementation, else (for a search that takes them) its declaration, then its
      -- `.lua`. A name it does not have falls through to `package.path`, for a library
      -- installed for the machine.
      if H.resolve_name then
         local kind, a, b, c = H.resolve_name(H.current_requirer(), module_name)
         if kind == "found" then
            local path = a or (search_all and (b or c)) or nil
            local fd = path and io.open(path, "rb")
            if fd then return path, fd, {} end
            return nil, nil, {}
         elseif kind == "hidden" then
            -- A dependency reaching the project's own module: the `tl.parse` wrapper
            -- reports it at the `require`, which is the one error to say. Answering with
            -- the file keeps Teal from adding a `module not found` in front of it.
            local fd = b and io.open(b, "rb")
            if fd then return b, fd, {} end
            return nil, nil, { a }
         elseif kind == "shadowed" then
            -- A name the host provides that a file of the model implements too: the
            -- `tl.parse` wrapper reports it at the `require`. The file is never the
            -- answer — the host's module is what runs — so the require is typed from the
            -- name's declaration when there is one, and otherwise answered with nothing,
            -- whose `module not found` the wrapper's error replaces.
            local fd = search_all and b and io.open(b, "rb")
            if fd then return b, fd, {} end
            return nil, nil, { a }
         elseif kind ~= "outside" then
            return nil, nil, { a }
         end
      end
      local found, fd, tried = tl_search(module_name, false) -- `.tl` only
      if not found and search_all then
         found, fd, tried = tl_search(module_name, true) -- `.d.tl`, then `.lua`
      end
      -- The model's own file, found under a name the model does not give it — what a
      -- `package.path` template makes of `src/util/util.tl` for `util`: not that module.
      if found and H.owns_file and H.owns_file(found) then
         if fd then fd:close() end
         tried = tried or {}
         tried[#tried + 1] = "'" .. found .. "' is not the module '" .. module_name .. "'"
         return nil, nil, tried
      end
      return found, fd, tried
   end
   tl.search_module = function(module_name, search_all)
      local found, fd, tried = search(module_name, search_all)
      -- A module the env will take from the store rather than walk gets its globals now,
      -- at the `require` — and no file handle: `tl.check_file` returns the seeded result
      -- before it would read one, and a handle it never takes is one nobody closes.
      if found and deliver_globals(found) and fd then
         fd:close()
         fd = nil
      end
      return found, fd, tried
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

-- Takes each side as the names that are on and the names that are off, and builds the two
-- maps here, for the reason `H.set_deps` below gives: the prelude is not always in the
-- state its caller holds (`Htl::with_checker`), and a table made on the other side of that
-- line cannot be passed across it.
--
-- The off names are carried and not dropped because absent and `false` differ here:
-- `H.tl_cfg[rule] ~= false` is what decides whether a Teal warning kind is said, so a kind
-- left out of the table would be said rather than silenced.
local function on_off(on, off)
   local t = {}
   for _, name in ipairs(on or {}) do t[name] = true end
   for _, name in ipairs(off or {}) do t[name] = false end
   return t
end

function H.set_lints(lua_on, lua_off, tl_on, tl_off)
   H.lint_cfg = on_off(lua_on, lua_off)
   H.tl_cfg = on_off(tl_on, tl_off)
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

-- The markers go where the record is declared, and the report lands where it is built,
-- so an SDK can declare the shape its mods must fill in. They are comments: the file
-- stays valid Teal and other tooling ignores them.
--
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
-- A field is required unless marked `---@optional`: the default is mandatory, which is
-- the opposite of `---@contract`'s (optional unless `---@required`; see contract.rs),
-- because this record is one the program builds itself and that one arrives from outside.
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

-- `---@async` and `async function`: whether the function called at (y, x) may suspend,
-- for the `await-missing` / `await-non-async` rules. Read off the declaring line the way
-- `---@nilable` is: the trailing marker on a `.d.tl` declaration (what `htl dts` and
-- `#[host_module]` write for an `async fn`, and what a hand-written one says), or the
-- keyword itself on a Teal `async function` -- the checker never saw it as a token
-- (`async_parse` takes it out), but the source line still shows it. What the position
-- report hands back for a callee is the function's own type, with the file and line of
-- its declaration: a `local async function f` reports its own line, a record method
-- `f: function(..)` the field's line, so an `async function R.f` whose record body
-- declares `f` is read at the body's line and needs the marker there.
local function async_at_decl(cache, t)
   local lines = source_lines(cache, t.file)
   if not lines then return false end
   if marker_on(lines, t.y, "async") then return true end
   local line = lines[t.y]
   return line ~= nil and line:find("%f[%w_]async%s+function%f[^%w_]") ~= nil
end

-- Resolver for the await rules: true when the function called at (y, x) is async. The
-- position is the callee's, as for `nilable_resolver`; a record is refused the same way.
local function async_resolver(result, filename)
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
      if marked[id] == nil then marked[id] = async_at_decl(sources, t) end
      return marked[id]
   end
end

-- `---@noyield(f, g)`: which parameters of a function its implementation calls from C, for
-- `async-as-sync-callback`. A host method that takes a Lua function and calls it with mlua's
-- `Function::call` runs it the way `table.sort` runs its comparator: a suspension inside it
-- fails with `attempt to yield across a C-call boundary`. Nothing in the type says so -- the
-- parameter is a bare `function` either way -- so the declaration says it, by name, the way
-- `---@async` says a function may suspend. Names and not slot numbers: `api:each(cb)` and
-- `api.each(api, cb)` put `cb` one argument apart, and the declaration's own parameter list
-- is what turns a name into the argument of either form.
--
-- The parameter list of the function type the position report puts at (y, x), names in
-- order: `f?: T` is `f`, an explicit `self: X` is `self`, `...: T` is `...`, and a Teal
-- `function R:m(..)` gets the `self` its `:` implies. The search starts at the type's own
-- column: every function type written on one line reports that line, so `mk: function(f:
-- function): function(a: string)` holds two, and the second is the one at its `function`
-- keyword. A list that does not close on its line is read on across the lines that follow
-- (at most `MAX_PARAM_LINES`), with comments dropped, so a declaration written one parameter
-- per line reads the same. The list is split at top-level commas only, so `f: function(a:
-- string, b: string)` is one parameter.
local MAX_PARAM_LINES = 32

local function declared_params(lines, y, x)
   local line = lines[y]
   if not line then return nil end
   local at = line:find("%f[%w_]function%f[^%w_]", x or 1)
   if not at then return nil end
   local text = line:sub(at + #"function"):gsub("%-%-.*$", "")
   local head, list = text:match("^([^(]*)(%b())")
   for i = 1, MAX_PARAM_LINES do
      if list or not lines[y + i] then break end
      text = text .. "\n" .. lines[y + i]:gsub("%-%-.*$", "")
      head, list = text:match("^([^(]*)(%b())")
   end
   if not list then return nil end
   local params = {}
   if head:match("^%s*[%w_.]+:[%w_]+") then params[1] = "self" end
   local depth, from, body = 0, 1, list:sub(2, -2)
   local function take(s)
      local name = s:match("^%s*([%w_]+)") or s:match("^%s*(%.%.%.)")
      if name then params[#params + 1] = name end
   end
   for i = 1, #body do
      local c = body:sub(i, i)
      if c == "(" or c == "{" or c == "<" then
         depth = depth + 1
      elseif c == ")" or c == "}" or c == ">" then
         depth = depth - 1
      elseif c == "," and depth == 0 then
         take(body:sub(from, i - 1))
         from = i + 1
      end
   end
   take(body:sub(from))
   return params
end

-- What `noyield_at` answers for a declaration that carries the marker with names:
-- { names = { "f" }, params = { "self", "f" } }, or false.
local function noyield_at_decl(cache, t)
   local lines = source_lines(cache, t.file)
   if not lines then return false end
   local found, args = marker_on(lines, t.y, "noyield")
   if not found or not args then return false end
   local names = {}
   for name in args:gmatch("[^,%s]+") do names[#names + 1] = name end
   local params = declared_params(lines, t.y, t.x)
   if #names == 0 or not params then return false end
   return { names = names, params = params }
end

-- Resolver for `async-as-sync-callback`: for the function called at (y, x), the parameters
-- its declaration marks `---@noyield(..)` and its own parameter list, or nil. The position is
-- the callee's, the one `async_resolver` is asked at; a record is refused the same way.
local function noyield_resolver(result, filename)
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
      if not id then return nil end
      local t = deref(id, 0)
      if not t or t.fields or not t.file or not t.y then return nil end
      if marked[id] == nil then marked[id] = noyield_at_decl(sources, t) end
      return marked[id] or nil
   end
end

-- `---@noyield` with no argument on a record field: the host reads the field off a table it
-- was handed and calls it from C -- `htl-mq`'s game table, whose `update` and `draw` run every
-- frame through `Function::call`. There is no callee in the Teal source to ask; the table
-- constructor is typed as the record, and the record type in the position report maps each
-- field to a type that carries the file and line it is declared on
-- (`TypeReporter:get_typenum`). The field's own type is read without following a nominal
-- `ref`: `update: Step` names the line `Step` is declared on once dereferenced, and the
-- marker sits on the field's line.
--
-- Resolver: (y, x) is the constructor's position, `key` the field it binds; the answer is
-- the record as its declaration names it (`api.Game`) when that field's line carries the
-- bare marker, or nil.
local function noyield_field_resolver(result, filename)
   local ok, report = pcall(tl.get_types, result)
   if not ok or type(report) ~= "table" then return nil end
   local by_pos = report.by_pos and report.by_pos[filename]
   if not by_pos then return nil end
   local sources, names = {}, {}
   local function deref(id, depth)
      local t = report.types[id]
      if t and t.ref and depth < 8 then return deref(t.ref, depth + 1) end
      return t
   end
   return function(y, x, key)
      local id = by_pos[y] and by_pos[y][x]
      if not id then return nil end
      local t = deref(id, 0)
      if not t or not t.fields or not t.fields[key] then return nil end
      local f = report.types[t.fields[key]]
      if not f or not f.file or not f.y then return nil end
      local lines = source_lines(sources, f.file)
      if not lines then return nil end
      local found, args = marker_on(lines, f.y, "noyield")
      if not found or args then return nil end
      if names[id] == nil then
         local rlines = t.file and t.y and source_lines(sources, t.file)
         names[id] = rlines and qualified_name(rlines, t.y, t.str or "record") or (t.str or "record")
      end
      return names[id]
   end
end

-- Resolver for lints: the checker's name for the type of the expression at (y, x)
-- (`"string"`, `"{string}"`, `"http"`, ...), nil when the report stored nothing there. For
-- `async-as-sync-callback`'s method form: `s:gsub(pat, f)` is `string.gsub` -- and calls `f`
-- from C -- only when `s` is a string; a `gsub` method on a record is a Teal function and may
-- take an async one.
local function type_name_resolver(result, filename)
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
      return t and t.str or nil
   end
end

-- The modules this check reached through `require`, with the AST each was parsed into:
-- what `await-outside-async` reads for an `await` at a module's top level. The file being
-- checked is the entry of `htl run` / `htl test` and its top level is async; a module is
-- loaded by `require`, which cannot yield, so its top level is not.
local function required_modules(result, env)
   local out = {}
   local names = {}
   for name in pairs(result.dependencies or {}) do names[#names + 1] = name end
   table.sort(names)
   for _, name in ipairs(names) do
      local fname = result.dependencies[name]
      local r = env and env.loaded and env.loaded[fname]
      if r and r.ast then out[#out + 1] = { filename = fname, ast = r.ast } end
   end
   return out
end

-- `---@extensible`: records a table may carry keys beyond the ones they declare. Every
-- Teal record is closed — a table typed as a record may not carry a key the record does
-- not declare, and that is a checker error rather than a lint, so no allow comment and no
-- `[lint]` setting reaches it. `---@optional` and `---@required` decide which *declared*
-- fields a literal may leave out; this marker is about the key the declaration has never
-- heard of. A value arriving from outside the program is where that costs: a mod written
-- against a newer SDK, a save file from a later version, a table a host will grow next
-- release, each carrying one key more than the declaration knows about, and without this
-- each is refused the way a mod that is *short* is refused — which leaves a lockstep edit
-- of every declaration as the only way for a producer to ship a new field. The marker
-- says the declaration is not the whole set, and the only thing it buys is that tl's
-- `unknown field <k>` is dropped for the keys it does not declare.
--
-- Read from the declaring file, in both forms — trailing, or on a line of its own above
-- the record — like the markers above and for the same reason: the checker discards
-- comments, and the file being checked is rarely the one that declares the record. A
-- record nested inside an extensible one is not extensible by that (`marker_on` reads
-- the record's own line); mark it too if it should be. An unmarked record stays closed,
-- as every record is: this returns nil for it.
--
-- The keys stay unreadable: `m.extra` through the record type is still an error. The
-- marker buys tolerance where a value is built and nothing else. A program that wants to
-- *read* what it did not declare wants a map field — `extra: {string: any}` — which is
-- the right answer when the keys are to be used and the wrong one at a data boundary,
-- since every producer then has to nest its extra keys under an agreed name, a change to
-- the wire shape rather than to the type.
--
-- Nothing here relaxes which *declared* fields a literal must set. `---@struct` and
-- `---@required` are answered elsewhere and are untouched by this, so a record can be
-- open at one end (keys nobody declared) and closed at the other (fields it does).
--
-- What it costs is one case: a misspelled *optional* field becomes silence. `colour` is
-- no longer an unknown field, and `struct-fields` has nothing to say because nothing is
-- missing -- the required case is still caught, the optional case is not. That is the
-- price of the marker rather than an oversight: a near-miss heuristic here would fire on
-- the very keys the marker exists to allow, and a warning that is wrong whenever the
-- marker is doing its job is worse than the silence.
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

-- The parts of a diagnostic, kept beside its text: what the Rust side reads instead of
-- taking the text apart again. `msg` is the sentence alone, without the position and
-- without the ` [htl <rule>]` suffix; `rule` names what said it (a lint, a Teal warning
-- kind, or for an error the class a fix is filed under).
local function item(filename, e, msg, rule)
   return { file = e.filename or filename, line = e.y or 0, col = e.x or 0, message = msg or e.msg or "?", rule = rule }
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
   local items = {}
   for _, w in ipairs(result.warnings or {}) do
      local rule = w.tag and ("tl:" .. w.tag)
      local file = w.filename or filename
      if not rule then
         out[#out + 1] = fmt(filename, w)
         items[#out] = item(filename, w)
      elseif H.tl_cfg[rule] ~= false and not allowed(file, w.y, rule) then
         out[#out + 1] = fmt(filename, w) .. " [htl " .. rule .. "]"
         items[#out] = item(filename, w, w.msg, rule)
      end
   end
   return out, items
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
   local before = H.asking
   H.asking = filename
   local found, fd = tl.search_module(name, true)
   H.asking = before
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
-- for an empty record). nil when the record's end line is unknown. The insertion is a
-- safe fix: a declaration line adds a field the record already has a definition for, so
-- what the program does at run time is unchanged, and the record becomes the module's
-- declared API. Moving the definition up is the other fix.
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

-- Every literal `require("<name>")` in `ast`, with where it resolves on the current path
-- unless `names_only` says the names are all that is wanted.
local function require_sites(ast, names_only, requirer)
   local out, seen = {}, {}
   local function go(n)
      if type(n) ~= "table" or seen[n] then return end
      seen[n] = true
      if type(n.kind) == "string" and n.kind == "op" and n.op and n.op.op == "@funcall"
         and type(n.e1) == "table" and n.e1.kind == "variable" and n.e1.tk == "require"
         and type(n.e2) == "table" and type(n.e2[1]) == "table" and n.e2[1].kind == "string" then
         local tk = n.e2[1].tk or ""
         local name = tk:sub(2, -2)
         local found
         if not names_only then
            local fd
            local before = H.asking
            H.asking = requirer or before
            found, fd = tl.search_module(name, true)
            H.asking = before
            if fd then fd:close() end
         end
         out[#out + 1] = { name = name, y = n.y, x = n.x, path = found, node = n.e2[1] }
      end
      for k, v in pairs(n) do
         if k ~= "if_parent" and k ~= "type" and k ~= "newtype" and k ~= "decltuple" and k ~= "expected"
            and type(v) == "table" then go(v) end
      end
   end
   go(ast)
   return out
end

-- Rewriting a `require` and holding a dependency to its own view both need the requiring
-- file, and every file a check reaches is parsed through `tl.parse` with its name — the
-- checked file and each one `require` leads to. The model decides both
-- (`H.rewrite_name`, `H.resolve_name`); this is where they are applied.
--
-- A rewrite goes into the AST, so what is checked and what is generated carry the same
-- name. A dependency reaching the project's own module is found when the file is parsed
-- and reported once it has been checked, as a type error of the dependency's file: a
-- parse error would stop the file being checked at all, and every module requiring it
-- would then see a type that is not its own.
local view_errors = {}

local function rewrite(site, to)
   site.node.conststr = to
   site.node.tk = string.format("%q", to)
end

---------------------------------------------------------------- async / await

-- `[lang] async = true` (`H.set_lang`) makes `async` and `await` keywords of the project's
-- Teal. Teal itself is not forked: the two words are found by `tl.lex`, taken out of the
-- token stream, and the AST `tl.parse_program` builds from the rest is rewritten at the
-- positions they were at. Every other token keeps its line and column, so the checker's
-- positions and the generated Lua's lines are the source's.
--
-- What each spelling becomes:
--
--   local async function f(..)  |  the function, marked `htl_async` (the checker rules read it)
--   global async function f     |  the same
--   async function R.f(..)      |  the same
--   async function(..) .. end   |  the same, an anonymous one
--   async local x = e           |  local x <close> = require("htl.task").of(e)   -- checked:
--                               |     `of<T>(v: T): Task<T>` types `x` from `e`, where a
--                               |     closure would not (Teal infers nothing through an
--                               |     untyped `function() return e end`)
--                               |  local x <close> = require("htl.task").spawn(function() return e end)
--                               |     -- generated: the closure is what runs as the task
--   await x   (x an async local)|  x:await()
--   await f(..)                 |  f(..), the call marked `htl_awaited`
--
-- The checked form and the generated form differ in one place, the `async local`, so the
-- AST is rewritten once for the check and the `of(e)` call is turned into the `spawn`
-- call just before `tl.generate` (`H.async_gen_ast`, from `H.gen` / `H.gen_string`). The
-- module is required inline at each use rather than through a `local` added at the top:
-- a line added would move every line after it.
--
-- Whether a word is the keyword is decided from its neighbours, since both are ordinary
-- names in Teal and in existing code: `async` is the keyword before `local` or
-- `function` and a name anywhere else; `await` is a name after `.` or `:` (a field, a
-- method) and before `:` `=` `,` `)` `.` `(` `]` `}` (a field being declared or set, an
-- argument, a call of a function so named), and the keyword otherwise.
H.lang_async = false

function H.set_lang(async_on)
   H.lang_async = async_on and true or false
end

local AWAIT_NAME_AFTER = { ["."] = true, [":"] = true }
local AWAIT_NAME_BEFORE = {
   [":"] = true, ["="] = true, [","] = true, [")"] = true, ["."] = true, ["("] = true,
   ["]"] = true, ["}"] = true,
}

local function is_node(v)
   return type(v) == "table" and type(v.kind) == "string" and v.y ~= nil
end

-- Numeric children in order, then named ones in a fixed order, so a walk is deterministic.
local function each_child(node, f)
   for i = 1, #node do
      local c = node[i]
      if is_node(c) then f(c, i) end
   end
   local keys = {}
   for k, v in pairs(node) do
      if type(k) == "string" and k ~= "op" and is_node(v) then keys[#keys + 1] = k end
   end
   table.sort(keys)
   for _, k in ipairs(keys) do f(node[k], k) end
end

-- Every node by (line, column), parents first, and each node's slot in its parent.
local function index_nodes(ast)
   local by_pos, parents = {}, {}
   local function visit(node, parent, key)
      if parents[node] then return end
      parents[node] = { parent = parent, key = key }
      local yl = by_pos[node.y]
      if not yl then yl = {}; by_pos[node.y] = yl end
      local xl = yl[node.x]
      if not xl then xl = {}; yl[node.x] = xl end
      xl[#xl + 1] = node
      each_child(node, function(c, k) visit(c, node, k) end)
   end
   visit(ast, nil, nil)
   return by_pos, parents
end

local raw_parse  -- the vendored `tl.parse`, set where the wrapper is installed

-- The expression of `local _ = <src>`, parsed by the plain parser, every node moved to
-- (y, x) — `yend` too, or the generator joins the lines after it (a node it emits `end`
-- for is placed at `yend`). `y_end` for the closure whose body ends on a later line.
local function template_expr(src, filename, y, x, y_end)
   local ast = raw_parse("local _ = " .. src, "htl-async-template", "tl")
   local e = ast[1].exps[1]
   local function move(node)
      node.y, node.x = y, x
      node.yend, node.xend = y_end or y, x
      node.f = filename
      each_child(node, move)
   end
   move(e)
   return e
end

-- Put `with` where the placeholder named `name` is in `root`.
local function graft(root, name, with)
   local done = false
   local function go(node)
      each_child(node, function(c, k)
         if done then return end
         if c.kind == "variable" and c.tk == name then
            node[k] = with
            done = true
         else
            go(c)
         end
      end)
   end
   go(root)
   assert(done, "htl: async template without its placeholder")
end

-- Which `variable` nodes name an `async local` in scope: those get `x:await()`. A plain
-- `local`, a function's arguments and a loop's variables shadow the name again.
local FUNCTION_KINDS = {
   local_function = true, global_function = true, record_function = true, ["function"] = true,
}

local function mark_task_vars(ast)
   local scopes = { {} }
   local depth = 0 -- functions entered; an `async local` remembers the depth it was declared at
   local function top() return scopes[#scopes] end
   local function lookup(name)
      for i = #scopes, 1, -1 do
         local v = scopes[i][name]
         if v ~= nil then return v end
      end
      return false
   end
   local function walk(node)
      if node.kind == "variable" and node.tk then
         local v = lookup(node.tk)
         if v then
            node.htl_task_var = true
            -- Read inside a function the declaring one contains: the task is captured,
            -- which `task-escape` reports.
            if depth > v.depth then node.htl_task_captured = true end
         end
      end
      if node.kind == "local_declaration" then
         if node.exps then walk(node.exps) end
         for _, v in ipairs(node.vars) do
            top()[v.tk] = node.htl_async_local and { depth = depth } or false
         end
         return
      elseif node.kind == "local_function" and node.name then
         top()[node.name.tk] = false
      end
      local pushed, entered = false, false
      if FUNCTION_KINDS[node.kind] then
         depth = depth + 1
         entered = true
      end
      if node.body or node.kind == "statements" then
         scopes[#scopes + 1] = {}
         pushed = true
         if node.args then
            for _, a in ipairs(node.args) do
               if a.tk then top()[a.tk] = false end
            end
         end
         if node.kind == "fornum" and node.var then top()[node.var.tk] = false end
         if node.kind == "forin" and node.vars then
            for _, v in ipairs(node.vars) do top()[v.tk] = false end
         end
      end
      each_child(node, walk)
      if pushed then scopes[#scopes] = nil end
      if entered then depth = depth - 1 end
   end
   walk(ast)
end

local POSTFIX = { ["@funcall"] = true, ["@index"] = true, ["."] = true, [":"] = true }

-- The AST of a file that used the two keywords: the rewrites above, at the positions the
-- lexer saw them. Errors go to `errs` in the parser's own shape, so they are reported as
-- syntax errors at the keyword.
local function apply_async_marks(ast, marks, errs, filename)
   local by_pos, parents = index_nodes(ast)
   local function at(y, x, kind)
      local l = by_pos[y] and by_pos[y][x]
      if not l then return nil end
      for _, n in ipairs(l) do
         if n.kind == kind then return n end
      end
   end
   local function err(m, msg)
      table.insert(errs, { filename = filename, y = m.y, x = m.x, msg = msg })
   end
   -- The statement begins at the keyword: what the formatter indents a block's first
   -- statement by (a block is positioned at its first statement), and where a
   -- diagnostic on the declaration points.
   local function start_at(node, x)
      node.x = x
      local slot = parents[node]
      if slot and slot.key == 1 and slot.parent and slot.parent.kind == "statements" then
         slot.parent.x = x
      end
   end
   local gen = {}
   for _, m in ipairs(marks) do
      if m.kind == "async_local" then
         local node = at(m.vy, m.vx, "local_declaration")
         if not node then
            err(m, "syntax error: 'async local' needs a local variable declaration after it")
         elseif #node.vars ~= 1 then
            err(m, "syntax error: 'async local' declares one name, not " .. #node.vars ..
               ": a task holds one value; write one 'async local' per task")
         elseif not node.exps or #node.exps ~= 1 then
            err(m, "syntax error: 'async local' takes one expression, the task's body")
         else
            local e = node.exps[1]
            node.vars[1].attribute = "close"
            node.htl_async_local = true
            node.htl_async_at = { y = m.y, x = m.x }
            -- The expression is the task's body and runs inside it, an async context of
            -- its own: a call of an async function there needs no `await` (`await-missing`
            -- skips calls so marked). A function written inside it is a context of its own.
            local function mark_body(n)
               if n.kind == "op" and n.op and n.op.op == "@funcall" then n.htl_task_body = true end
               if not FUNCTION_KINDS[n.kind] then each_child(n, mark_body) end
            end
            mark_body(e)
            local call = template_expr('require("htl.task").of(__E)', filename, m.y, m.x)
            graft(call, "__E", e)
            node.exps[1] = call
            start_at(node, m.x)
            gen[#gen + 1] = { call = call, e = e }
         end
      elseif m.kind == "async_function" then
         local node = (m.ly and at(m.ly, m.lx, "local_function"))
            or at(m.fy, m.fx, "global_function")
            or at(m.fy, m.fx, "record_function")
            or at(m.fy, m.fx, "function")
         if node then
            node.htl_async = true
            if not m.ly then start_at(node, m.x) end
         else
            err(m, "syntax error: 'async' goes before 'local function', 'global function', " ..
               "'function R.f' or 'function(...)'")
         end
      end
   end
   mark_task_vars(ast)
   for _, m in ipairs(marks) do
      if m.kind == "await" then
         -- The operand is the postfix chain that starts at the token after `await`: the
         -- innermost node there (a name, a parenthesis) and every call / index / method
         -- built on it upward, stopping at a binary operator (`await f(b) + 1` awaits
         -- `f(b)`). A call node is positioned at its `(`, not at the name, so the chain is
         -- climbed from the name rather than looked up by position.
         local target
         local list = by_pos[m.oy] and by_pos[m.oy][m.ox]
         for i = #(list or {}), 1, -1 do
            local n = list[i]
            if n.kind == "variable" or n.kind == "paren" or n.kind == "string" or
               n.kind == "table" or n.kind == "number" or n.kind == "identifier" then
               target = n
               break
            end
         end
         while target do
            local slot = parents[target]
            local p = slot and slot.parent
            if p and p.kind == "op" and POSTFIX[p.op.op] and p.e1 == target then
               target = p
            else
               break
            end
         end
         if not target then
            err(m, "syntax error: 'await' needs a call after it: await f(x)")
         elseif target.kind == "variable" then
            if target.htl_task_var then
               local call = template_expr("__X:await()", filename, target.y, target.x)
               graft(call, "__X", target)
               call.htl_awaited = true
               call.htl_task_await = true
               call.htl_await_at = { y = m.y, x = m.x }
               local slot = parents[target]
               slot.parent[slot.key] = call
            else
               err(m, "'await' on '" .. tostring(target.tk) .. "', which is not an async local: " ..
                  "an async function is awaited at its call (await f(x)), a task at the name " ..
                  "an 'async local' gave it")
            end
         elseif target.kind == "op" and target.op.op == "@funcall" then
            target.htl_awaited = true
            target.htl_await_at = { y = m.y, x = m.x }
         elseif target.kind == "paren" then
            err(m, "syntax error: 'await (..)': put 'await' inside the parentheses, before the call")
         else
            err(m, "syntax error: 'await' applies to a call: await f(x)")
         end
      end
   end
   ast.htl_async_gen = gen
end

-- The keyword tokens of `input`, by the rule in the header, and the tokens without them.
local function split_async_tokens(tokens)
   local marks, kept = {}, {}
   for i, t in ipairs(tokens) do
      local prev, nxt = tokens[i - 1], tokens[i + 1]
      local keep = true
      if t.kind == "identifier" and t.tk == "async" then
         if nxt and nxt.tk == "local" then
            local var = tokens[i + 2]
            marks[#marks + 1] = { kind = "async_local", y = t.y, x = t.x,
               vy = var and var.y, vx = var and var.x }
            keep = false
         elseif nxt and nxt.tk == "function" then
            local m = { kind = "async_function", y = t.y, x = t.x, fy = nxt.y, fx = nxt.x }
            if prev and (prev.tk == "local" or prev.tk == "global") then m.ly, m.lx = prev.y, prev.x end
            marks[#marks + 1] = m
            keep = false
         end
      elseif t.kind == "identifier" and t.tk == "await" then
         local named = (prev and AWAIT_NAME_AFTER[prev.tk]) or (nxt and AWAIT_NAME_BEFORE[nxt.tk])
         if not named then
            marks[#marks + 1] = { kind = "await", y = t.y, x = t.x, oy = nxt and nxt.y, ox = nxt and nxt.x }
            keep = false
         end
      end
      if keep then kept[#kept + 1] = t end
   end
   return marks, kept
end

-- `tl.parse` under `[lang] async`: lex, take the keywords out, parse, rewrite.
local function async_parse(input, filename, parse_lang)
   local tokens, errs = tl.lex(input, filename)
   local marks, kept = split_async_tokens(tokens)
   local ast, required = tl.parse_program(kept, errs, filename, parse_lang)
   if ast and #marks > 0 then
      apply_async_marks(ast, marks, errs, filename)
   end
   return ast, errs, required
end

-- Turn the checked form of every `async local` into the generated one (header), once.
function H.async_gen_ast(ast)
   for _, g in ipairs(ast and ast.htl_async_gen or {}) do
      if not g.done then
         g.done = true
         g.call.e1.e2.tk = "spawn"
         local fn = template_expr("function() return __E end", g.call.f, g.call.y, g.call.x, g.e.yend or g.e.y)
         graft(fn, "__E", g.e)
         g.call.e2[1] = fn
      end
   end
end

do
   local tl_parse = tl.parse
   raw_parse = tl_parse
   tl.parse = function(input, filename, parse_lang)
      local ast, errs, required
      if H.lang_async then
         ast, errs, required = async_parse(input, filename, parse_lang)
      else
         ast, errs, required = tl_parse(input, filename, parse_lang)
      end
      if ast and filename and H.rewrite_name then
         for _, site in ipairs(require_sites(ast, true)) do
            local to = H.rewrite_name(filename, site.name)
            if to then rewrite(site, to) end
         end
         for _, site in ipairs(require_sites(ast, true)) do
            local kind, msg = H.resolve_name(filename, site.name)
            if kind == "hidden" or kind == "ambiguous" or kind == "shadowed" then
               view_errors[filename] = view_errors[filename] or {}
               table.insert(view_errors[filename], {
                  filename = filename, y = site.y, x = site.x, msg = msg,
                  -- Nothing answered the search for it — for a name the host provides,
                  -- when it has no declaration — so Teal says `module not found` at the
                  -- same place: this error is instead of that one.
                  replaces = (kind == "ambiguous" or kind == "shadowed")
                     and ("module not found: '" .. site.name .. "'") or nil,
               })
            end
         end
      end
      return ast, errs, required
   end
   local tl_check_string = tl.check_string
   tl.check_string = function(input, env, filename, parse_lang)
      -- The file whose `require`s Teal resolves while this runs, and the env it resolves
      -- them into.
      table.insert(H.requirers, filename)
      table.insert(H.envs, env)
      local ok, result = pcall(tl_check_string, input, env, filename, parse_lang)
      table.remove(H.envs)
      table.remove(H.requirers)
      if not ok then error(result, 0) end
      local pending = filename and view_errors[filename]
      if pending and result then
         view_errors[filename] = nil
         result.type_errors = result.type_errors or {}
         for _, e in ipairs(pending) do
            if e.replaces then
               for i = #result.type_errors, 1, -1 do
                  local t = result.type_errors[i]
                  if t.y == e.y and t.x == e.x and t.msg == e.replaces then
                     table.remove(result.type_errors, i)
                  end
               end
            end
            table.insert(result.type_errors, { filename = e.filename, y = e.y, x = e.x, msg = e.msg })
         end
         result.ok = false
      end
      return result
   end
end

-- Proactive form of the same check: every `require("<literal>")` in the file whose
-- resolution is the file itself gets its own error at the call site. Teal may swallow
-- the self-require as a circular require and only complain later ("unknown type
-- site.Config"), which hides the cause.
local function self_require_errors(filename, ast)
   local out = {}
   for _, r in ipairs(require_sites(ast, false, filename)) do
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

-- What a record body's first line may not be called, and the two spellings that work.
--
-- The sentence keeps the `syntax error:` prefix Teal's own parse errors carry, because
-- that prefix is how the rest of htl recognises a file the parser rejected (`htl fix`
-- refuses to rewrite one), and because it is still true: the parse failed.
local WHERE_FIELD_MSG =
   "syntax error: 'where' opens a union predicate when it is the first line of a record " ..
   "or interface body; write [\"where\"]: <type>, or put another field first"

-- Explain the bare "syntax error" a field named `where` produces, and drop the parse
-- errors that follow from it.
--
-- `where` is not one of Teal's keywords -- the lexer hands it back as an identifier --
-- but `parse_record_body` reads an optional `where <expression>` clause, the predicate
-- that discriminates a union variant (it is stored as the record's `__is` metamethod),
-- after the body's array-interface and `is` lists and *before* its field loop starts. So
-- `where: any` on the body's first line is parsed as that clause's expression, dies on
-- the `:` that follows, and is reported as a bare "syntax error" naming neither `where`
-- nor the field. One line further down the field loop has begun and `where` is an
-- ordinary field name; `["where"]` takes the field loop's bracketed-string-literal
-- branch and is accepted in any position, first line included. Both of those type-check
-- today, so what the message has to name is the position, not the name -- "`where` is
-- reserved in a record body" would be false.
--
-- Spotting the candidate is a line match, but being wrong about it would put a
-- confident, misleading sentence on an unrelated parse failure, so the guess is proved
-- before it is printed: the line is rewritten to the bracketed spelling and the file
-- parsed again. The message is swapped in only when that rewrite clears the error, and
-- the errors the parser goes on to report on later lines are dropped only when the same
-- rewrite clears them too -- which is what "a consequence of this one" means here,
-- rather than a rule about adjacency. The price is one extra parse (no type check) of a
-- file that has already failed to parse, and none at all for a file that has not.
--
-- Returns msgs, dropped, both keyed by index into `syntax_errors`: `msgs[i]` replaces
-- that error's text, `dropped[i]` removes the error.
local function explain_where_field(filename, src, syntax_errors)
   local msgs, dropped = {}, {}
   if not src then return msgs, dropped end
   local lines = {}
   for line in (src .. "\n"):gmatch("([^\n]*)\n") do lines[#lines + 1] = line end
   for i, e in ipairs(syntax_errors) do
      local line = e.msg == "syntax error" and e.y and lines[e.y]
      local indent, gap = nil, nil
      if line then indent, gap = line:match("^(%s*)where(%s*):") end
      -- The error has to point at that very colon; anything else on the line is a
      -- different failure that happens to sit next to a `where`.
      if indent and e.x == #indent + 5 + #gap + 1 then
         local rewritten = {}
         for j, l in ipairs(lines) do rewritten[j] = l end
         rewritten[e.y] = indent .. "[\"where\"]" .. line:sub(#indent + 6)
         local _, errs = tl.parse(table.concat(rewritten, "\n"), filename, "tl")
         local still = {}
         for _, r in ipairs(errs or {}) do still[r.y or 0] = true end
         if not still[e.y] then
            msgs[i] = WHERE_FIELD_MSG
            local j = i + 1
            while syntax_errors[j] and syntax_errors[j].y and syntax_errors[j].y > e.y
               and not still[syntax_errors[j].y] do
               dropped[j] = true
               j = j + 1
            end
         end
      end
   end
   return msgs, dropped
end

-- The methods declared directly in `record <name>` in a Teal declaration's text, in the
-- order they are written.
--
-- A line scan rather than a parse: the only caller reads `htl/test.d.tl`, a file this
-- crate ships, and running the parser over it to answer one error message would cost
-- more than the message is worth. `record`, `enum` and `interface` openers are counted
-- so that a nested one cannot end the scan early, and only depth 1 contributes names --
-- a method of a nested record is not a method of this one.
local function record_method_names(src, name)
   local out = {}
   local depth
   for line in (src .. "\n"):gmatch("([^\n]*)\n") do
      if not depth then
         -- `record Expect<T>` or a bare `record SnapshotConfig`; the anchors keep
         -- `Expect` from matching the line that opens `Expect2`.
         if line:match("^%s*record%s+" .. name .. "%s*<") or line:match("^%s*record%s+" .. name .. "%s*$") then
            depth = 1
         end
      elseif line:match("^%s*end%s*$") then
         depth = depth - 1
         if depth == 0 then break end
      elseif line:match("^%s*record%s") or line:match("^%s*enum%s") or line:match("^%s*interface%s") then
         depth = depth + 1
      elseif depth == 1 then
         local m = line:match("^%s*([%a_][%w_]*)%s*:%s*function%s*%(")
         if m then out[#out + 1] = m end
      end
   end
   return out
end

-- The matchers a misspelt one is missing from, appended to Teal's own message.
--
-- `t.expect(#params):to_be(2)` fails as `invalid key 'to_be' in type Expect<integer>`.
-- That names the key and the type and stops: the reader is told the name is wrong and
-- not one name that is right. The set is in the README's "Tests" section, but the moment
-- the checker refuses a matcher is exactly the moment nobody goes to look, so the set
-- travels with the message.
--
-- It is not written out here. `htl/test.d.tl` is the declaration the call was checked
-- against, and `result.dependencies` holds the very file tl resolved this module's
-- `require("htl.test")` to, so the names are read back out of that file: one source of
-- truth, and a matcher added to the declaration is in the message the same day. The file
-- is read at most once per checked file and only when such an error exists, which is why
-- this is a closure over the result rather than a table built up front.
--
-- Requiring the dependency is also what keeps the hint honest: a project with a generic
-- `Expect` record of its own, in a file that never requires `htl.test`, gets Teal's text
-- unchanged, because those matchers are not the ones it is missing.
local function matcher_lister(result)
   local known = {} -- record name -> "a, b, c", or false when there is nothing to say
   return function(type_name)
      local list = known[type_name]
      if list == nil then
         list = false
         local decl = (result.dependencies or {})["htl.test"]
         local fd = decl and io.open(decl, "rb")
         if fd then
            local names = record_method_names(fd:read("a"), type_name)
            fd:close()
            if #names > 0 then list = table.concat(names, ", ") end
         end
         known[type_name] = list
      end
      return list or nil
   end
end

local function collect_errors(filename, result, src)
   -- A result served again from the env cache (every runtime `require` of a module
   -- already checked) would otherwise re-walk its AST for require sites and re-resolve
   -- each one on disk: ~11 ms per module, ~1.4 s over a 261-test run [measured].
   if result.htl_errors and result.htl_errors_for == filename then
      return result.htl_errors, result.htl_error_fixes, result.htl_error_items
   end
   local errors = {}
   -- error_fixes[i] = fix for errors[i], or false: a rewrite `htl fix` may apply.
   local error_fixes = {}
   -- error_items[i] = the parts of errors[i] (`item`), its rule the class its fix is filed
   -- under: `forward-ref`, or `tl:error` for anything else the checker said.
   local error_items = {}
   result.htl_errors, result.htl_error_fixes, result.htl_errors_for = errors, error_fixes, filename
   result.htl_error_items = error_items
   -- The file's text, read from disk once and only when something below asks for it: the
   -- caller has it on some paths and not on others, and a check whose file both parses
   -- and requires nothing suspicious never needs it.
   local function source()
      if src == nil then
         local fd = io.open(filename, "rb")
         src = fd and fd:read("a") or false
         if fd then fd:close() end
      end
      return src or nil
   end
   local syntax_errors = result.syntax_errors or {}
   local where_msgs, where_dropped = {}, {}
   if #syntax_errors > 0 then
      where_msgs, where_dropped = explain_where_field(filename, source(), syntax_errors)
   end
   for i, e in ipairs(syntax_errors) do
      if not where_dropped[i] then
         errors[#errors + 1] =
            fmt(filename, { filename = e.filename, y = e.y, x = e.x, msg = where_msgs[i] or e.msg })
         error_items[#errors] = item(filename, e, where_msgs[i] or e.msg, "tl:error")
      end
   end
   if result.ast and #syntax_errors == 0 then
      -- Cheap text prefilter: only when some `require("<name>")` in the source resolves
      -- to this very file is the AST walked for exact positions. The walk costs tens of
      -- ms on a large module and it ran for every module a program required [measured].
      local suspicious = false
      for name in (source() or ""):gmatch("require%s*%(?%s*[\"']([^\"']+)[\"']") do
         local before = H.asking
         H.asking = filename
         local found, fd = tl.search_module(name, true)
         H.asking = before
         if fd then fd:close() end
         if found and norm_path(found) == norm_path(filename) then suspicious = true break end
      end
      if suspicious then
         for _, e in ipairs(self_require_errors(filename, result.ast)) do
            errors[#errors + 1] = fmt(filename, e)
            error_items[#errors] = item(filename, e, e.msg, "tl:error")
         end
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
   local matchers_of = matcher_lister(result)
   for _, e in ipairs(result.type_errors or {}) do
      local msg = explain_self_require(filename, e)
      local own = e.filename == nil or e.filename == filename
      local fix
      local class = "tl:error"
      if own then
         local explained = explain_arity(result.ast, e, msg)
         if explained ~= msg then hinted[e.y] = true end
         msg, fix = explain_forward_ref(result.ast, src, e, explained)
         -- Said here, where it is known, so nothing reads it back out of the sentence.
         if msg ~= explained then class = "forward-ref" end
      end
      -- `invalid key 'to_be' in type Expect<integer>`: the assertion library's own type
      -- said no, so the answer is the set of names it says yes to. Teal's text stays the
      -- prefix -- anything matching on it keeps matching.
      local expect = msg:match("^invalid key '[%w_]+' in type (Expect%d*)%s*<")
      if expect then
         local list = matchers_of(expect)
         if list then msg = msg .. "; the matchers are " .. list .. " (README, \"Tests\")" end
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
         error_items[#errors] = item(filename, e, msg, class)
      end
   end
   -- syntax / self-require errors carry no fix
   for i = 1, #errors do
      if error_fixes[i] == nil then error_fixes[i] = false end
   end
   return errors, error_fixes, error_items
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

-- The names a checked file's top level declares `global`, or nil when it declares none:
-- the three node kinds that reach `TypeChecker:add_global` (vendor/tl.lua), and the name
-- each carries (`global_declaration` is `global a, b = ...`, one name per var). The scan is
-- the one `checked_enums` does a few lines above, over the same top-level list and for the
-- same reason -- a declaration at the top level of the file is the whole of what a module
-- contributes to the env's globals.
local function declared_globals(result)
   local names
   local function add(name_node, node)
      names = names or {}
      -- The name's own position where the parser kept one (an identifier node), else
      -- the statement's: what `global-redeclaration` points at.
      names[#names + 1] = { name = name_node.tk, y = name_node.y or node.y, x = name_node.x or node.x }
   end
   for _, node in ipairs(result.ast or {}) do
      local k = node.kind
      if k == "global_type" then
         add(node.var, node)
      elseif k == "global_function" then
         add(node.name, node)
      elseif k == "global_declaration" then
         for _, var in ipairs(node.vars) do add(var, node) end
      end
   end
   return names
end

-- Checked-module store, shared by every fresh env in this state. A fresh env per file
-- exists so that module *names* resolve under that file's own search path and never
-- leak from another directory; the store keeps that guarantee by seeding an env only
-- with entries whose name still resolves to the very same file here. What is shared is
-- the result of checking a file, which does not depend on who required it. A file is
-- walked once per run, whoever required it first, and every record its walk declared is
-- one instance from then on -- which is what lets a value typed by one requirer be passed
-- to a function typed by another (Teal compares records by instance, not by shape).
--
-- A module that declares a `global` has one effect beyond its type: the walk registers
-- the name into the env it happened in, and replaying a result is not a walk -- so an env
-- seeded with such a module's result would never have its global. Walking the file again
-- in every requiring env (what #302 did) gives every env the global and every env its own
-- record instances, which is the split above. So the walk stays one, and the entry keeps
-- what the walk registered (`globals`, the very var tables from that env), for
-- `deliver_globals` (top of this file) to put into each env that requires the module.
-- That happens at the `require` and not at the seed, because a global is visible where
-- the module was required and nowhere else: seeding it into every env would let a file
-- read a name whose declaration it never required, and pass.
--
-- The same holds one level down. A walk of `modx` runs its `require("host")`, so a file
-- that requires `modx` and never names `host` still sees `host`'s globals -- that is what
-- a `require` chain means in Lua, and what the checker of a whole project in one env
-- would see. A `modx` served from the store runs no requires, so its requirer is handed
-- the globals of everything below it too (`globals_below`), and every module with a
-- global anywhere below it is seeded the way a declaring module is (`seed_env`), so that
-- its require reaches the point of delivery.
local store = {} -- module name -> { filename, type, result, globals, sites, closure, closure_sites }

local function store_from(env)
   for name, ty in pairs(env.modules) do
      local fname = env.module_filenames[name]
      local result = fname and env.loaded[fname]
      -- skip the placeholder tl leaves while a module is being checked (circular requires)
      if result and result.type == ty then
         local e = { filename = fname, type = ty, result = result }
         local names = declared_globals(result)
         if names then
            -- The var tables of the walk that produced this result. A later env that was
            -- served the result holds the same tables (delivered), or -- when it walked a
            -- declaration of its own for one of the names first -- another declaration's;
            -- the entry keeps the ones the walk made, not whatever this env has now.
            local old = by_file[fname]
            if old and old.result == result and old.globals then
               e.globals = old.globals
            else
               e.globals = {}
               for _, n in ipairs(names) do e.globals[n.name] = env.globals[n.name] end
            end
            -- Where each was declared, for `global-redeclaration` (`sites_below`).
            e.sites = {}
            for _, n in ipairs(names) do
               e.sites[#e.sites + 1] = { name = n.name, file = fname, y = n.y, x = n.x }
            end
         end
         by_file[fname] = e
         store[name] = e
      end
   end
end

-- A module with a global anywhere below it -- one its own top level declares, or one a
-- module it requires (transitively) declares -- is seeded by its result only:
-- `env.loaded`, not `env.modules` nor `env.module_filenames`. `require_module`
-- (vendor/tl.lua) answers from `env.modules[name]` when it is there and never reaches
-- `tl.search_module`, and the search is where `deliver_globals` hands the env those
-- globals. Left out of `env.modules`, the require falls through to the search, the
-- wrapper delivers, and `tl.check_file` returns the seeded result without reading the
-- file: no walk, one record instance, the globals present. A module with no global below
-- it is seeded whole, as before, and its require never leaves `require_module`.
--
-- (The `.htl/` cache on the Rust side needs nothing for this: its module entries are
-- stamped with `checker_identity()`, a hash over this file, so a warm cache misses of its
-- own accord the moment this changes.)
local function seed_env(env)
   for name, e in pairs(store) do
      if env.modules[name] == nil then
         local found, fd = tl.search_module(name, true)
         if fd then fd:close() end
         if found == e.filename then
            if next(globals_below(e)) == nil then
               env.modules[name] = e.type
               env.module_filenames[name] = e.filename
            end
            env.loaded[e.filename] = e.result
         end
      end
   end
end

function H.reset_store()
   store = {}
   for k in pairs(by_file) do by_file[k] = nil end
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
               local errs, _, items = collect_errors(fname, dep)
               for i, text in ipairs(errs) do
                  out[#out + 1] = { file = fname, required_by = requirer, text = text, item = items[i] }
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
   local errors, error_fixes, error_items = collect_errors(filename, result)
   local warnings, warning_items = warnings_of(filename, result)
   local deps = {}
   for _, fname in pairs(result.dependencies or {}) do deps[#deps + 1] = fname end
   table.sort(deps)
   local lints, lint_fixes, lint_items = {}, {}, {}
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
            type_at = type_name_resolver(result, filename),
            async_at = H.lang_async and async_resolver(result, filename) or nil,
            noyield_at = H.lang_async and noyield_resolver(result, filename) or nil,
            noyield_field_at = H.lang_async and noyield_field_resolver(result, filename) or nil,
            lang_async = H.lang_async,
            required = H.lang_async and required_modules(result, env) or nil,
            deps = H.deps,
            union_at = union_resolver(result, filename),
            cast_at = cast_at,
            enum_table_at = enum_table_at,
         })
         prof("lint.run", filename, t1)
         for _, l in ipairs(found or {}) do
            lints[#lints + 1] = fmt(filename, l)
            lint_fixes[#lints] = l.fix or false
            lint_items[#lints] = item(filename, l, l.bare, l.rule)
         end
      end
   end
   local requires = {}
   -- require sites feed the project-level require-cycle lint: same gate as the lints.
   if opts.lints ~= false and result.ast then requires = require_sites(result.ast, false, filename) end
   prof("lint+req", filename, t0)
   -- Errors in what this file required. `ok` stays the file's own answer: `H.gen` still
   -- generates it, and the searcher refuses the dependency itself on its first `require`.
   local dep_errors = dependency_errors(filename, result, env)
   -- Every `global` declaration this check brought into scope, and where: the file's own,
   -- then those of its require closure through the store (`sites_below`). The file itself
   -- is in the store only once something required it, so its own are read off the result.
   local global_sites, seen = {}, {}
   for _, n in ipairs(declared_globals(result) or {}) do
      global_sites[#global_sites + 1] = { name = n.name, file = filename, y = n.y, x = n.x }
      seen[filename .. ":" .. n.y .. ":" .. n.x] = true
   end
   local dep_names = {}
   for name in pairs(result.dependencies or {}) do dep_names[#dep_names + 1] = name end
   table.sort(dep_names)
   for _, name in ipairs(dep_names) do
      local d = by_file[result.dependencies[name]]
      if d then
         for _, s in ipairs(sites_below(d)) do
            local key = s.file .. ":" .. s.y .. ":" .. s.x
            if not seen[key] then
               seen[key] = true
               global_sites[#global_sites + 1] = s
            end
         end
      end
   end
   return { ok = #errors == 0, errors = errors, error_fixes = error_fixes, warnings = warnings, deps = deps,
      lints = lints, lint_fixes = lint_fixes, requires = requires, dependency_errors = dep_errors,
      error_items = error_items, warning_items = warning_items, lint_items = lint_items,
      global_sites = global_sites,
      syntax_errors = #(result.syntax_errors or {}), result = result }
end

-- `H.check` of what is on disk right now: a fresh env, nothing seeded, nothing stored --
-- the two options `Htl::check_written` is for. They are set here rather than handed over
-- as a table, so that nothing built in the caller's state has to cross into this one (see
-- `H.set_deps`).
function H.check_written(filename)
   return H.check(filename, nil, { seed = false, store = false })
end

-- A file that checked but did not generate: the one error it has is that, in both forms a
-- check carries (`errors[i]` and its parts `error_items[i]`), so a reader of either sees the
-- same one. It is about the whole file, so it sits at 1:1 as htl's other findings about a
-- file do, and the path is the item's `file` rather than words in the message: a reader
-- spells it (`Diagnostic::spelled`) as it spells every other.
local function generate_failed(c, filename, gerr)
   local msg = "generate failed: " .. tostring(gerr)
   c.errors = { filename .. ":1:1: " .. msg }
   c.error_fixes = {}
   c.error_items = { { file = filename, line = 1, col = 1, message = msg } }
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
   H.async_gen_ast(c.result.ast)
   local code, gerr = tl.generate(c.result.ast, H.GEN_TARGET)
   prof("generate", filename, t0)
   -- Terminated here, at the producer. `tl.generate` joins one output line per input line
   -- and writes nothing after the last, and five consumers read what it returns (`htl gen`,
   -- `include_tl!`, `--source` bundles, the run cache, the mlua-pkg resolver) of which only
   -- the CLI used to append the byte, so one generator emitted two shapes. A file ends with
   -- exactly one newline: that is what POSIX means by a line, and every one of the five
   -- hands the string on as a file. Before the caching below, so a replay carries it too.
   if code and code:sub(-1) ~= "\n" then code = code .. "\n" end
   if code then c.result.htl_code = code end
   if not code then
      c.ok = false
      generate_failed(c, filename, gerr)
      return nil, c
   end
   return code, c
end

-- Type-check + generate from source text (used by the mlua-pkg resolver, where the
-- sandbox already read the file). Same return shape as H.gen.
function H.gen_string(src, filename)
   local result = tl.check_string(src, H.env, filename)
   local errors, error_fixes, error_items = collect_errors(filename, result, src)
   local warnings = warnings_of(filename, result, src)
   local c = {
      ok = #errors == 0, errors = errors, error_fixes = error_fixes, error_items = error_items,
      warnings = warnings, deps = {}, lints = {}, result = result,
   }
   if not c.ok or not result.ast then
      return nil, c
   end
   H.async_gen_ast(result.ast)
   local code, gerr = tl.generate(result.ast, H.GEN_TARGET)
   if not code then
      c.ok = false
      generate_failed(c, filename, gerr)
      return nil, c
   end
   -- Terminated as `H.gen` terminates it, for the reason given there: the resolver's
   -- caller is one of the five, and a second shape would be a second answer.
   if code:sub(-1) ~= "\n" then code = code .. "\n" end
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

-- Literal `require`s of a plain Lua file (a vendored dependency), resolved like the
-- checker resolves them. Parsed with tl in Lua mode; a file tl cannot parse yields
-- no sites (its requires are then the host's to declare).
function H.lua_requires(src, filename)
   local ast, errs = tl.parse(src, filename, "lua")
   if not ast or (errs and #errs > 0) then return {} end
   return require_sites(ast, false, filename)
end

-- The module names a Teal source `require`s by literal, read from its syntax alone: no
-- type check and no resolution, so it is cheap enough to ask of every file in a tree.
-- nil when the source does not parse, which is not the same answer as "requires
-- nothing".
function H.tl_require_names(src, filename)
   local ast, errs = tl.parse(src, filename)
   if not ast or (errs and #errs > 0) then return nil end
   local names = {}
   for _, site in ipairs(require_sites(ast, true)) do
      names[#names + 1] = site.name
   end
   return names
end

-- Where `require(name)` would resolve for the checker (`.tl` / `.d.tl` / `.lua`), and
-- where a plain `.lua` implementation sits on the path, if any. Both may be nil.
-- A name more than one file implements resolves to neither: the model's search answers
-- nothing for it, and `H.ambiguity` says why. A name the host provides that a file of the
-- model implements as well (kind `shadowed`) resolves to its declaration, or to nothing,
-- and never to a `.lua`: the file is not what runs, so nothing downstream may take it.
function H.resolve_module(name)
   local kind, _, _, lua = nil, nil, nil, nil
   if H.resolve_name then kind, _, _, lua = H.resolve_name(nil, name) end
   local found, fd = tl.search_module(name, true)
   if fd then fd:close() end
   local lua_path
   if kind == "found" then
      -- The model's `.lua` for the name, and no other: `package.path` would also find one
      -- under a name the model does not give it.
      lua_path = lua
   elseif kind == nil or kind == "outside" then
      lua_path = package.searchpath(name, package.path)
   end
   return found, lua_path
end

-- The model's message when more than one file implements `name`, else nil.
function H.ambiguity(name)
   if not H.resolve_name then return nil end
   local kind, msg = H.resolve_name(nil, name)
   if kind == "ambiguous" then return msg end
   return nil
end

-- The model's message when the host provides `name` and a file of the model implements
-- it too (kind `shadowed`), else nil. `H.resolve_module` answers such a name with its
-- declaration or nothing, which alone reads as a host module or a missing one; this is
-- what tells the linker it is neither, for a `require` no check has seen (a plain `.lua`).
function H.host_shadowing(name)
   if not H.resolve_name then return nil end
   local kind, msg = H.resolve_name(nil, name)
   if kind == "shadowed" then return msg end
   return nil
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
-- templates a directory contributes (`add_path` adds two or three).
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

-- Every `<name>.d.tl` there is: with the project model (`H.claims_name`), every
-- declaration a module of the model has under the name, the project's first; without
-- one, every one reachable on the current `package.path`, in the order the path is
-- consulted — the declarations of `module_candidates`, which is the same walk.
function H.declaration_sites(name)
   local out = {}
   if H.claims_name then
      for _, p in ipairs(H.claims_name(name)) do
         if p:sub(-5) == ".d.tl" then out[#out + 1] = p end
      end
      return out
   end
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

-- Type-check a stub in a fresh env: the shared env caches module types by name, so a
-- second `Site` (another contract dir) would be judged by the first one's type.
function H.check_stub(src, filename)
   local result = tl.check_string(src, new_env(), filename)
   return collect_errors(filename, result, src)
end

-- Static contract check for one module file (the `contract` lint):
--   1. `local m: <type_path> = require("<modname>")` through the checker (type errors),
--   2. with require_fields (`true` for every declared field, or a list of names): keys
--      of the module's returned table literal vs those. The literal is found through
--      `return { … }`, `return define({ … })`, `return { … } as T`, and
--      `local m: T = { … } … m.f = … return m` (see the `as` / local cases below).
-- Returns { errors = {string}, missing = {string} | nil (nil = not decidable),
--           bad_require_fields = {string} (names the type does not declare) }.
function H.contract_check(filename, modname, type_path, require_fields)
   local module = type_path:match("^([^.]+)%.")
   local out = { errors = {}, missing = nil }
   if not module then
      out.errors[1] = "contract type must be written as <module>.<Type>: " .. tostring(type_path)
      return out
   end
   local stub = string.format('local %s = require("%s")\nlocal m: %s = require("%s")\nreturn m\n',
      module, module, type_path, modname)
   -- The sentences alone: the stub's own position says nothing about the module, and the
   -- caller points at the module instead.
   local _, _, items = H.check_stub(stub, "<contract " .. type_path .. " for " .. modname .. ">")
   for i, it in ipairs(items or {}) do out.errors[i] = it.message end
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
   -- A module the project model has as `.lua` alone (a dependency's plain Lua, typed or
   -- not) is served from here, at the file the model says: Lua's own searcher reads
   -- `package.path`, which has never heard of `@<dependency>/…` and would read a template's
   -- alias as readily as the file.
   if H.resolve_name then
      local kind, impl, _, lua = H.resolve_name(nil, module_name)
      -- Two implementations: the check reports it at the `require`, but a `require` the
      -- check never saw (in a plain `.lua`) reaches here, and `tl.search_module` would
      -- answer it with one of the two. No order picks one at run time either.
      -- A name the host provides that a file implements too is refused the same way: a
      -- run with the host never reaches this searcher (`package.preload` answers first),
      -- so serving the file here would run what no run with the host runs.
      if kind == "ambiguous" or kind == "shadowed" then error(impl, 0) end
      if kind == "found" and not impl and lua then
         local lfd = io.open(lua, "rb")
         if lfd then
            local src = lfd:read("a")
            lfd:close()
            return "code", src, lua
         end
      end
   end
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
--
-- The templates of a directory are the naming rule's spellings (`htl_core::naming`):
-- `?.lua` and `?/init.lua` everywhere, and `?/?.lua` only where the directory holds
-- packages by name (`packages` true: the dependency links, the parent of a vendored copy
-- or a patch), because `<name>/<name>.tl` is a flat package's entry and nothing else. On
-- any other directory the template made `util/util.tl` answer to `util` as well as to
-- `util.util`.
function H.templates(dir, packages)
   local t = dir .. "/?.lua;" .. dir .. "/?/init.lua"
   if packages then t = t .. ";" .. dir .. "/?/?.lua" end
   return t
end
--
-- A directory already on the path keeps the place it has. Whoever put it there said
-- where it goes -- a host stating its order once with add_search_paths, or an earlier
-- add_path -- and moving it to the front would make the search order depend on who
-- called last: a resolver that puts its own root in front on its first resolve would
-- override the host for every module checked after it. Absent from the path, the
-- directory is still prepended, so the only source of a root is still consulted first.
function H.add_path(dir, packages)
   local templates = H.templates(dir, packages)
   if package.path == nil or package.path == "" then
      package.path = templates
      return
   end
   -- Whole entries, not a substring: "/a/?.lua" must not match "/other/a/?.lua". The
   -- first template stands for the rest, which are only ever written together here.
   local first = dir .. "/?.lua"
   for entry in package.path:gmatch("[^;]+") do
      if entry == first then
         return
      end
   end
   package.path = templates .. ";" .. package.path
end

-- Put `dir` in front of the path for the length of one check, whatever else is on it,
-- and return the path it replaced so the caller can put it back with H.set_path.
--
-- Unlike add_path this does move a directory already on the path, and that is the point:
-- the caller is not stating a search order, it is naming the one directory whose copy of
-- a module the next check is about. `TealResolver`'s expect_type stub is the caller --
-- it resolves the served module by name, and a second contract directory holding a module
-- of the same name would otherwise decide which file the contract is checked against.
function H.push_path_front(dir, packages)
   local saved = package.path
   local templates = H.templates(dir, packages)
   if saved == nil or saved == "" then
      package.path = templates
   else
      package.path = templates .. ";" .. saved
   end
   return saved
end

-- Drop Lua's default search path (`./?.lua` etc., i.e. cwd-relative resolution) so only
-- directories given to add_path are consulted. Used by the proc macros, where the cwd
-- is cargo's and has nothing to do with the script being embedded.
-- Drop the entries of package.path that are relative to the working directory (Lua's own
-- `./?.lua;./?/init.lua`), keeping the rest in order.
function H.drop_cwd_path()
   local kept = {}
   for entry in (package.path or ""):gmatch("[^;]+") do
      if entry:sub(1, 2) ~= "./" and entry:sub(1, 1) ~= "?" then
         kept[#kept + 1] = entry
      end
   end
   package.path = table.concat(kept, ";")
end

function H.reset_path()
   package.path = ""
end

return H
