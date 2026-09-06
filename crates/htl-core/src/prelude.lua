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

H.lint_cfg = lint.DEFAULT

-- `+rule,-rule,...` on top of the defaults. Returns nil, err on unknown rule.
function H.set_lints(spec)
   local cfg, err = lint.config(spec)
   if not cfg then return nil, err end
   H.lint_cfg = cfg
   return true
end

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
      if not (own and hinted[e.y] and msg:find("(unresolved generic)", 1, true)) then
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
   local warnings = {}
   for _, w in ipairs(result.warnings or {}) do warnings[#warnings + 1] = fmt(filename, w) end
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
         local found = lint.run(src, filename, H.lint_cfg, {
            enums = enums,
            subject_enum = subject,
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
   return { ok = #errors == 0, errors = errors, error_fixes = error_fixes, warnings = warnings, deps = deps,
      lints = lints, lint_fixes = lint_fixes, requires = requires, result = result }
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
   local warnings = {}
   for _, w in ipairs(result.warnings or {}) do warnings[#warnings + 1] = fmt(filename, w) end
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
--   2. with require_fields: keys of the module's returned table literal (also
--      `X.define({ ... })`) vs the record's declared fields (missing ones).
-- Returns { errors = {string}, missing = {string} | nil (nil = not decidable) }.
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
   for _, f in ipairs(declared) do
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
