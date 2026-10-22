-- htl runtime_version: the `_VERSION` a module states about itself, read off its syntax tree.
--
-- What it answers: which lshape the project supplies, to compare with the lshape its
-- declaration was written for (`htl::shapes::runtime_version`, which is the Rust side of
-- this file). lshape states its own version where a Lua library conventionally
-- does, as a field named `_VERSION`, and this finds that field without running the module.
-- The project's Lua is parsed, never loaded: running a dependency to ask it a question
-- would run its top level, and a check must not do that.
--
-- No pattern matching on the text. A regular expression finds `_VERSION = "1"` inside a
-- comment, a long string and a second table just as well as in the assignment that means
-- it; the tree has only the real ones. Two shapes count, and nothing else:
--
--   * an assignment to a field:            `M._VERSION = "0.3.0"`
--   * a field of a table constructor:      `return { _VERSION = "0.3.0" }`
--
-- A local named `_VERSION` is not a field and is not read, and neither is
-- `M["_VERSION"]`: both are rarer than the two forms above, and a form that is read is a
-- form lshape has to keep writing. A library that states its version some other way gets
-- "could not read it" from the check, which says so and names the file.
--
-- Whose `_VERSION`: the module's own, which is the field of the value the chunk returns.
-- Only the chunk's top-level statements are read, and the last top-level `return` decides:
--
--   * `return { _VERSION = "..." , ... }`: that constructor's field;
--   * `return M`: the field of `M`, from `local M = { _VERSION = "..." }` at the top level
--     and then from each top-level `M._VERSION = "..."`, the last one winning, since that
--     is the value the module hands out when it is loaded;
--   * anything else (no `return`, `return f()`, a name never given the field): missing.
--
-- So a nested table with its own `_VERSION` (`local compat = { _VERSION = "5.1" }`), a
-- helper table's field and a `_VERSION` inside a function body are not the module's, and
-- are not read: taking the first in source order would report a lshape the project does
-- not have.
--
-- What this depends on: the shape of the vendored `tl.lua`'s nodes, which is why it is
-- tested against a parse (`tests/shapes.rs`) rather than described. An assignment is kind
-- `assignment` with `vars` and `exps`; a field access `a.b` is kind `op` with `op.op ==
-- "."`, `e1` and `e2` (an `identifier` whose `tk` is the name); a constructor's entry is
-- `literal_table_item` with `key_parsed == "short"`, a `key` whose `conststr` is the name,
-- and a `value`; a string literal is kind `string` with `conststr` its content.
local V = {}

local FIELD = "_VERSION"

-- What a value expression says: "found" with the string's content when it is a string
-- literal, "not_literal" for anything else (a call, a concatenation, a variable, a number)
-- and for no value at all.
local function literal(value)
   if type(value) == "table" and value.kind == "string" and type(value.conststr) == "string" then
      return "found", value.conststr
   end
   return "not_literal"
end

-- The `_VERSION` entry of a table constructor, its own entries only: { kind, text } or nil.
local function in_constructor(node)
   if type(node) ~= "table" or node.kind ~= "literal_table" then return nil end
   local found
   for _, item in ipairs(node) do
      if type(item) == "table" and item.kind == "literal_table_item"
         and item.key_parsed == "short" and type(item.key) == "table"
         and item.key.conststr == FIELD then
         local kind, text = literal(item.value)
         found = { kind = kind, text = text }
      end
   end
   return found
end

-- Whether `var` is `<name>._VERSION`.
local function is_field_of(var, name)
   return type(var) == "table" and var.kind == "op" and type(var.op) == "table"
      and var.op.op == "." and type(var.e1) == "table" and var.e1.tk == name
      and type(var.e2) == "table" and var.e2.tk == FIELD
end

-- The version a parsed module states: "found", text | "not_literal" | "missing".
function V.read(ast)
   if type(ast) ~= "table" then return "missing" end
   local ret
   for _, stmt in ipairs(ast) do
      if type(stmt) == "table" and stmt.kind == "return" then ret = stmt end
   end
   local value = ret and ret.exps and ret.exps[1]
   if type(value) ~= "table" then return "missing" end
   local direct = in_constructor(value)
   if direct then return direct.kind, direct.text end
   if value.kind ~= "variable" and value.kind ~= "identifier" then return "missing" end
   local name = value.tk
   local found
   for _, stmt in ipairs(ast) do
      if type(stmt) ~= "table" then
         -- not a statement
      elseif stmt.kind == "local_declaration" and type(stmt.vars) == "table" then
         for i, var in ipairs(stmt.vars) do
            if type(var) == "table" and var.tk == name then
               found = in_constructor(stmt.exps and stmt.exps[i])
            end
         end
      elseif stmt.kind == "assignment" and type(stmt.vars) == "table" then
         for i, var in ipairs(stmt.vars) do
            if is_field_of(var, name) then
               local kind, text = literal(stmt.exps and stmt.exps[i])
               found = { kind = kind, text = text }
            end
         end
      end
   end
   if not found then return "missing" end
   return found.kind, found.text
end

return V
