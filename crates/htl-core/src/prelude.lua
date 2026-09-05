-- htl prelude: thin Lua-side layer over tl.lua (Teal compiler).
-- Loaded into every htl Lua state after `tl` is registered in package.preload.

local tl = require("tl")
local lint = require("htl.lint")
local fmt_mod = require("htl.fmt")
local H = {}

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
   return assert(tl.new_env({
      defaults = {
         feat_lax = "off",
         gen_compat = "off",
         gen_target = H.GEN_TARGET,
      },
   }), "htl: tl.new_env failed")
end

H.env = new_env()

local function fmt(filename, e)
   return string.format("%s:%d:%d: %s", e.filename or filename, e.y or 0, e.x or 0, e.msg or "?")
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
function H.check(filename)
   local env = new_env() -- bind first: assert() would also pass its message along as `fd`
   local result, err = tl.check_file(filename, env)
   if not result then
      return { ok = false, errors = { tostring(err) }, warnings = {} }
   end
   local errors, warnings = {}, {}
   for _, e in ipairs(result.syntax_errors or {}) do errors[#errors + 1] = fmt(filename, e) end
   for _, e in ipairs(result.type_errors or {}) do errors[#errors + 1] = fmt(filename, e) end
   for _, w in ipairs(result.warnings or {}) do warnings[#warnings + 1] = fmt(filename, w) end
   local deps = {}
   for _, fname in pairs(result.dependencies or {}) do deps[#deps + 1] = fname end
   table.sort(deps)
   local lints = {}
   if result.ast and #(result.syntax_errors or {}) == 0 then
      local src
      local fd = io.open(filename, "rb")
      if fd then src = fd:read("a"); fd:close() end
      if src then
         local found = lint.run(src, filename, H.lint_cfg, checked_enums(result, env))
         for _, l in ipairs(found or {}) do lints[#lints + 1] = fmt(filename, l) end
      end
   end
   return { ok = #errors == 0, errors = errors, warnings = warnings, deps = deps, lints = lints, result = result }
end

-- Type-check + generate Lua source. Returns code, checkinfo (code is nil on failure).
function H.gen(filename)
   local c = H.check(filename)
   if not c.ok then
      return nil, c
   end
   local code, gerr = tl.generate(c.result.ast, H.GEN_TARGET)
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
   local errors = {}
   for _, e in ipairs(result.syntax_errors or {}) do errors[#errors + 1] = fmt(filename, e) end
   for _, e in ipairs(result.type_errors or {}) do errors[#errors + 1] = fmt(filename, e) end
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
   local code, c = H.gen(found)
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
   package.path = dir .. "/?.lua;" .. dir .. "/?/init.lua;" .. dir .. "/?/?.lua;" .. package.path
end

return H
