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

local function collect_errors(filename, result)
   local errors = {}
   for _, e in ipairs(result.syntax_errors or {}) do errors[#errors + 1] = fmt(filename, e) end
   if result.ast and #(result.syntax_errors or {}) == 0 then
      for _, e in ipairs(self_require_errors(filename, result.ast)) do errors[#errors + 1] = fmt(filename, e) end
   end
   local hinted = {} -- lines where an arity error was explained by a multi-value call
   for _, e in ipairs(result.type_errors or {}) do
      local msg = explain_self_require(filename, e)
      local own = e.filename == nil or e.filename == filename
      if own then
         local explained = explain_arity(result.ast, e, msg)
         if explained ~= msg then hinted[e.y] = true end
         msg = explained
      end
      -- tl follows the arity error with "argument N: got X, expected T (unresolved
      -- generic)" for the very same call: a consequence, not a second mistake.
      if not (own and hinted[e.y] and msg:find("(unresolved generic)", 1, true)) then
         errors[#errors + 1] = fmt(filename, { filename = e.filename, y = e.y, x = e.x, msg = msg })
      end
   end
   return errors
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
-- opts.seed = false checks with a cold env (no store).
function H.check(filename, env, opts)
   opts = opts or {}
   local fresh = env == nil
   env = env or new_env() -- bind first: assert() would also pass its message along as `fd`
   local t0 = os.clock()
   if fresh and opts.seed ~= false then seed_env(env) end
   local result, err = tl.check_file(filename, env)
   prof("check", filename, t0)
   if result then store_from(env) end
   if not result then
      return { ok = false, errors = { tostring(err) }, warnings = {} }
   end
   t0 = os.clock()
   local errors, warnings = collect_errors(filename, result), {}
   for _, w in ipairs(result.warnings or {}) do warnings[#warnings + 1] = fmt(filename, w) end
   local deps = {}
   for _, fname in pairs(result.dependencies or {}) do deps[#deps + 1] = fname end
   table.sort(deps)
   local lints = {}
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
         for _, l in ipairs(found or {}) do lints[#lints + 1] = fmt(filename, l) end
      end
   end
   local requires = {}
   -- require sites feed the project-level require-cycle lint: same gate as the lints.
   if opts.lints ~= false and result.ast then requires = require_sites(result.ast) end
   prof("lint+req", filename, t0)
   return { ok = #errors == 0, errors = errors, warnings = warnings, deps = deps, lints = lints,
      requires = requires, result = result }
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
   local t0 = os.clock()
   local code, gerr = tl.generate(c.result.ast, H.GEN_TARGET)
   prof("generate", filename, t0)
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
   local errors = collect_errors(filename, result)
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
function H.check_stub(src, filename)
   local result = tl.check_string(src, new_env(), filename)
   return collect_errors(filename, result)
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

-- Strict searcher: unlike tl.loader(), type errors are fatal at require time.
local function strict_searcher(module_name)
   local found, fd = tl.search_module(module_name, false)
   if not found then
      -- Declaration-only module (`name.d.tl` with no `.lua` behind it): hand
      -- require an empty table so type-only requires do not fail at runtime.
      -- If a `.lua` exists, step aside for Lua's own searcher.
      local dfound, dfd = tl.search_module(module_name, true)
      if dfound and dfound:match("%.d%.tl$") then
         dfd:close()
         local lua_path = dfound:gsub("%.d%.tl$", ".lua")
         local init_path = dfound:gsub("%.d%.tl$", "/init.lua")
         local lf = io.open(lua_path, "rb") or io.open(init_path, "rb")
         if lf then
            lf:close()
            return "\n\ttype-only '" .. dfound .. "' (implementation served by the .lua searcher)"
         end
         return function() return H.type_only_module(module_name, dfound) end, dfound
      elseif dfd then
         dfd:close()
      end
      return "\n\tno .tl module '" .. module_name .. "' on package.path"
   end
   fd:close()
   -- Type errors are fatal here; lints are the CLI's business (`htl check`), not require's.
   local code, c = H.gen(found, { lints = false })
   if not code then
      error(table.concat(c.errors, "\n"), 0)
   end
   local chunk, lerr = load(code, "@" .. found, "t")
   if not chunk then
      error("htl: generated Lua failed to load: " .. tostring(lerr), 0)
   end
   return function(modname)
      return chunk(modname, found)
   end, found
end

function H.install_searcher()
   table.insert(package.searchers, 2, strict_searcher)
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
