-- htl search: the part of `require` resolution the checker and a program state both do
-- the same way, whichever state they live in.
--
-- #318: these functions used to be two copies, one in the checker prelude (`prelude.lua`,
-- `H.*`) and one in the runtime prelude Rust embeds for a split program state
-- (`RUNTIME_PRELUDE` in `lib.rs`, `R.*`). The copies matched exactly, but nothing kept
-- them matching: #313 changed the templates both built and had to edit both by hand. One
-- definition, evaluated directly into both the checker (`Htl::from_lua`) and a split
-- runtime state (`Htl::with_checker_lua`) and handed to whichever prelude loads next as
-- that chunk's own argument, is the fix. Not `package.preload["htl.search"]`: that name is
-- a project's to `require` for its own module (nothing reserves it), and a shared state
-- (`htl run`) would otherwise answer that `require` with this table instead, while a split
-- state (`htl test`, which never preloads this) would not — the two states disagreeing on
-- what a project's own name resolves to.
--
-- What this chunk may depend on: nothing of the checker's. No `tl`, no `H`, no project
-- model — a split runtime state never has any of those, and loading this before the
-- checker prelude does (`from_lua`) or without it at all (`with_checker_lua`) has to work
-- either way. Only what the runtime doc of `Htl::with_checker_lua` promises every program
-- state has — `package` (the searcher and `preload`), the base library's `load` and
-- `xpcall` — plus base-library functions (`setmetatable`, `getmetatable`, `rawget`,
-- `rawset`, `error`, `type`, `tostring`) and `string` / `table`, which every program state
-- this runs in has always had on top of that doc's minimum (the runtime prelude used them
-- before this chunk existed). `debug` is read defensively (`required_from_teal`) and its
-- absence is handled, not required.
--
-- The frame-walk invariant `required_from_teal` relies on: every function below is a
-- local of *this* chunk, so `debug.getinfo(1, "S").source` taken inside it is this
-- chunk's own name (`=htl-search` in both the checker and a split runtime state, set by
-- whoever loads this file) — and so is every frame the walk has to step over on its way
-- up to the `require` that asked (the closure `S.searcher` returns, `guard_loaded`'s
-- `__index`). Moving a function out of this chunk without moving the ones it is walked
-- past with would break the walk's "my own frames" test; keeping them together is why
-- they are one chunk and not several.
local S = {}

-- Value handed to a Teal `require` for a declaration-only module (`name.d.tl` with no
-- implementation on the path). Indexing it explains what is missing instead of the bare
-- "attempt to call a nil value" that would surface otherwise.
--
-- Only Teal is handed it (the searcher `S.searcher` returns, `required_from_teal`). A
-- checked `.tl` was typed against the declaration, so a table that stands for the module
-- until something touches it is the declaration's promise kept as far as this run can
-- keep it: an engine that calls its host in one function loads, and is tested, without
-- the host. A plain `.lua` was never checked and sees no declaration — to it the name is
-- not a module, and `pcall(require, name)`, Lua's one way to ask, has to say so.
--
-- The metatable carries `htl_declaration = true` so the stand-in is known by
-- construction: `S.install_searcher`'s `package.loaded` metatable keeps it out of
-- `package.loaded` and hands it back to Teal only (`guard_loaded`).
function S.type_only_module(module_name, decl_path)
   return setmetatable({}, {
      htl_declaration = true,
      __index = function(_, key)
         error(string.format(
            "module '%s' is declaration-only here (%s): '%s' has no implementation on this path. " ..
            "It must be provided by the host program (e.g. a Rust #[host_module] via cargo run) " ..
            "or by a .tl/.lua module with that name.",
            module_name, decl_path, tostring(key)), 2)
      end,
   })
end

-- Whether the `require` that reached a searcher was written in Teal: the source name of
-- the chunk that called it ends in `.tl`. Every chunk `htl run` / `htl test` make from a
-- `.tl` is named so — the searcher `S.searcher` returns, `R.preload_generated`, the test
-- runner and the entry (`@<path>.tl`) — while Lua's own searcher names a `.lua`
-- `@<path>.lua`.
--
-- The caller is the first frame above the searcher that is neither C nor this chunk's
-- own: `require` itself is C, and a C frame past it is `pcall` / `xpcall` or the like
-- passing the call through, not the chunk that wrote it. So `pcall(require, x)` in a
-- `.lua` is Lua's, and the same line in a `.tl` is Teal's: the split is by the language of
-- the chunk, not by how it called. A Lua helper requiring on a Teal caller's behalf is the
-- first Lua frame and is Lua, which is what it is. A chunk under any other name (a host's
-- own label, a stripped payload's `?`) or no Lua frame at all counts as Lua: the
-- conservative side, an error naming the declaration rather than a table that says the
-- module is there.
--
-- A state without the `debug` library (a host that sandboxed it) cannot see who asked and
-- answers `true`, keeping the table every caller got before this rule.
local function required_from_teal()
   local getinfo = type(debug) == "table" and debug.getinfo
   if type(getinfo) ~= "function" then return true end
   local own = getinfo(1, "S").source
   local level = 2
   while true do
      local info = getinfo(level, "S")
      if not info then return false end
      if info.what ~= "C" and info.source ~= own then
         return type(info.source) == "string" and info.source:sub(-3) == ".tl"
      end
      level = level + 1
   end
end

-- Keep the stand-in out of `package.loaded`. `require` stores what a loader returned in
-- `package.loaded` and answers every later `require` of the name from there, before any
-- searcher runs, so a stand-in a Teal `require` received would be handed to a plain-Lua
-- `require` after it: `pcall(require, name)` in a `.lua` would say `true` or `false` by
-- which file happened to ask first. A metatable on `package.loaded` moves the stand-in
-- aside instead. `require` reads and writes `package.loaded` through metamethods, and a
-- stand-in is never there raw, so both go through these two:
--
-- - `__newindex` stores a stand-in (`htl_declaration` on its metatable) in `side`, keyed by
--   name, and anything else raw, dropping the name's side entry: a real module stored later
--   (a `package.preload` loader's value, a host's own assignment, a `nil` that unloads it)
--   answers every caller from then on.
-- - `__index` hands the side entry back only when the read came from Teal
--   (`required_from_teal`, walking up from this function's frame as it walks up from the
--   searcher's) and nothing in `package.preload` answers the name, which a loader
--   registered after the stand-in (`preload_value`) does; otherwise `nil`, so a `.lua`'s
--   `require` goes on to the searchers and is declined, and a `.lua` reading
--   `package.loaded[name]` sees nothing.
--
-- A `package.loaded` that already has a metatable is the host's, and is left alone: that
-- run keeps the first caller's answer for every later one.
local function guard_loaded()
   local pkg = package
   local loaded = type(pkg) == "table" and pkg.loaded
   if type(loaded) ~= "table" or getmetatable(loaded) ~= nil then return end
   local side = {}
   setmetatable(loaded, {
      __newindex = function(t, k, v)
         local mt = type(v) == "table" and getmetatable(v)
         if type(mt) == "table" and rawget(mt, "htl_declaration") == true then
            side[k] = v
         else
            side[k] = nil
            rawset(t, k, v)
         end
      end,
      __index = function(_, k)
         local v = side[k]
         if v == nil then return nil end
         local preload = rawget(pkg, "preload")
         if type(preload) == "table" and preload[k] ~= nil then return nil end
         if required_from_teal() then return v end
         return nil
      end,
   })
end

-- The strict searcher, parameterized over how it asks what a `require` should do:
--
--   gen(name) -> kind, a, b
--     "code", code, found    generated Lua for a type-checked `.tl`: load it under the
--                             name `found`, as the searcher's own loader would.
--     "type_only", dfound    declaration-only module (`name.d.tl`, no `.lua` behind it).
--     anything else, msg     not found here: `msg` alone, a searcher's way of saying so.
--
--   decline(name, decl) -> the text a plain-Lua `require` of a declaration fails with —
--     the `Registry`'s trailing searcher's text, so the checker and a split runtime state
--     say the same thing (`Htl::install_searcher` builds the one function both are given).
--
-- The checker's `gen` is `resolve_for_require` in the checker prelude; a split runtime
-- state's is the Rust bridge `Htl::install_searcher` builds over the checker's own.
function S.searcher(gen, decline)
   return function(module_name)
      local kind, a, b = gen(module_name)
      if kind == "code" then
         local chunk, lerr = load(a, "@" .. b, "t")
         if not chunk then
            error("htl: generated Lua failed to load: " .. tostring(lerr), 0)
         end
         return function(modname) return chunk(modname, b) end, b
      elseif kind == "type_only" then
         -- Teal gets the table (see `S.type_only_module`); plain Lua gets Lua's answer: a
         -- searcher that returns a string has not found the module, so `require` fails
         -- with the text and `pcall(require, …)` is `false`, as through a `Registry`.
         if not decline or required_from_teal() then
            return function() return S.type_only_module(module_name, a) end, a
         end
         return decline(module_name, a)
      end
      return a
   end
end

-- Install `S.searcher(gen, decline)` at `package.searchers` position 2 — ahead of Lua's
-- own, behind `package.preload`, so a preloaded module is never asked of it — and guard
-- `package.loaded` behind it (`guard_loaded`). A fresh closure per call: `gen` and
-- `decline` are this call's, captured by the closure `S.searcher` returns, not read from
-- a shared upvalue, so two states (or two calls on the same one) never answer from each
-- other's `gen`.
function S.install_searcher(gen, decline)
   table.insert(package.searchers, 2, S.searcher(gen, decline))
   guard_loaded()
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
function S.templates(dir, packages)
   local t = dir .. "/?.lua;" .. dir .. "/?/init.lua"
   if packages then t = t .. ";" .. dir .. "/?/?.lua" end
   return t
end

-- A directory already on the path keeps the place it has. Whoever put it there said
-- where it goes -- a host stating its order once with add_search_paths, or an earlier
-- add_path -- and moving it to the front would make the search order depend on who
-- called last: a resolver that puts its own root in front on its first resolve would
-- override the host for every module checked after it. Absent from the path, the
-- directory is still prepended, so the only source of a root is still consulted first.
function S.add_path(dir, packages)
   local templates = S.templates(dir, packages)
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

-- Drop the entries of package.path that are relative to the working directory (Lua's own
-- `./?.lua;./?/init.lua`), keeping the rest in order.
function S.drop_cwd_path()
   local kept = {}
   for entry in (package.path or ""):gmatch("[^;]+") do
      if entry:sub(1, 2) ~= "./" and entry:sub(1, 1) ~= "?" then
         kept[#kept + 1] = entry
      end
   end
   package.path = table.concat(kept, ";")
end

-- Drop Lua's default search path (`./?.lua` etc., i.e. cwd-relative resolution) so only
-- directories given to add_path are consulted. Used by the proc macros, where the cwd is
-- cargo's and has nothing to do with the script being embedded.
function S.reset_path()
   package.path = ""
end

return S
