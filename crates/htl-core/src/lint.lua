-- htl lints: extra checks on the Teal syntax tree that Teal itself does not perform.
-- Lints run on a fresh `tl.parse` of the source (pure syntax, independent of the checker).
--
--   nil-index        `t[k].x` / `t[k]:m()` / `t[k]()` / `t[k][j]` -- indexing a map/array
--                    yields V, not V|nil in Teal, so chaining on it can raise at runtime.
--   enum-exhaustive  `if e == "a" then ... elseif e == "b" then ... end` where the string
--                    literals belong to a declared enum: every value must be covered or an
--                    `else` branch must exist.
--   shadow-local     a local (or loop / parameter name) reuses the name of a local in an
--                    enclosing scope.
--   no-global        `global` declarations (prefer locals + module return).
--   no-any           explicit `any` in annotations or `as any` casts.   [off by default]
--   explicit-number  unannotated local initialized with a numeric literal (`local n = 0`
--                    infers integer, `0.0` infers number); ask for the annotation. [off]
--   class-record     record declaring metamethods (a class): its metatable is not part of
--                    the value, so serialization and the Rust boundary drop it.     [off]
--
-- Suppress per line with a trailing comment:  -- htl: allow(nil-index, shadow-local)

local tl = require("tl")
local L = {}

L.DEFAULT = {
   ["nil-index"] = true,
   ["enum-exhaustive"] = true,
   ["shadow-local"] = true,
   ["no-global"] = true,
   ["no-any"] = false,
   ["explicit-number"] = false,
   ["class-record"] = false,
}

local SKIP_KEYS = { if_parent = true, type = true, newtype = true, decltuple = true, expected = true }

local function is_node(t)
   return type(t) == "table" and type(t.kind) == "string"
end

-- Generic walk over syntax nodes (types are skipped), each table visited once.
local function walk(root, visit)
   local seen = {}
   local function go(n)
      if type(n) ~= "table" or seen[n] then return end
      seen[n] = true
      if is_node(n) then visit(n) end
      for k, v in pairs(n) do
         if not SKIP_KEYS[k] and type(v) == "table" then go(v) end
      end
   end
   go(root)
end

local function unquote(tk)
   if type(tk) ~= "string" then return nil end
   local q = tk:sub(1, 1)
   if (q == '"' or q == "'") and tk:sub(-1) == q then
      return tk:sub(2, -2)
   end
   return nil
end

-- Serialize a "subject" expression (variable / dotted chain) to a stable key.
local function subject_key(n)
   if not is_node(n) then return nil end
   if n.kind == "variable" or n.kind == "identifier" then return n.tk end
   if n.kind == "op" and n.op and n.op.op == "." then
      local a, b = subject_key(n.e1), subject_key(n.e2)
      if a and b then return a .. "." .. b end
   end
   if n.kind == "paren" then return subject_key(n.e1) end
   return nil
end

local function collect_allows(src)
   local allows = {}
   local y = 0
   for line in (src .. "\n"):gmatch("(.-)\n") do
      y = y + 1
      local names = line:match("%-%-%s*htl:%s*allow%(([%w%-, ]+)%)")
      if names then
         allows[y] = allows[y] or {}
         for name in names:gmatch("[%w%-]+") do allows[y][name] = true end
      end
   end
   return allows
end

---------------------------------------------------------------- nil-index

local CHAIN_OPS = { ["."] = true, [":"] = true, ["@funcall"] = true, ["@index"] = true }
local CHAIN_WHAT = { ["."] = "field access", [":"] = "method call", ["@funcall"] = "call", ["@index"] = "index" }

local function lint_nil_index(ast, report)
   walk(ast, function(n)
      if n.kind == "op" and n.op and CHAIN_OPS[n.op.op] then
         local base = n.e1
         if is_node(base) and base.kind == "op" and base.op and base.op.op == "@index" then
            report("nil-index", n.y, n.x,
               CHAIN_WHAT[n.op.op] .. " directly on an index result: the value may be nil at runtime; bind it to a local and nil-check first")
         end
      end
   end)
end

---------------------------------------------------------------- enum-exhaustive

local function collect_enums(ast)
   local enums = {}
   walk(ast, function(n)
      if (n.kind == "local_type" or n.kind == "global_type") and is_node(n.value)
         and n.value.newtype and n.value.newtype.def and n.value.newtype.def.typename == "enum" then
         local name = n.var and n.var.tk or "?"
         enums[name] = n.value.newtype.def.enumset or {}
      end
   end)
   return enums
end

-- Flatten `e == "a" or e == "b"` into (subject, {"a","b"}, subject node); nil if not that shape.
local function literal_tests(exp)
   if not is_node(exp) or exp.kind ~= "op" then return nil end
   local op = exp.op and exp.op.op
   if op == "or" then
      local s1, l1, n1 = literal_tests(exp.e1)
      local s2, l2 = literal_tests(exp.e2)
      if s1 and s2 and s1 == s2 then
         for _, v in ipairs(l2) do l1[#l1 + 1] = v end
         return s1, l1, n1
      end
      return nil
   end
   if op == "==" then
      local lit = unquote(exp.e2 and exp.e2.tk)
      local subj, node = subject_key(exp.e1), exp.e1
      if lit == nil then
         lit = unquote(exp.e1 and exp.e1.tk)
         subj, node = subject_key(exp.e2), exp.e2
      end
      if lit and subj then return subj, { lit }, node end
   end
   return nil
end

-- `extra.enums`: name -> enumset the checker resolved (nested in records, required
-- modules). `extra.subject_enum(y, x, key)`: the checker's type of a subject —
-- (enumset, name) for an enum, `false` for a known non-enum, nil when unknown.
-- `if` statements that have statements after them in their block: when every branch
-- ends in `return`, what follows is the implicit `else`, not a missing branch.
local function mark_fallthrough(ast)
   local has_next = {}
   walk(ast, function(n)
      if n.kind ~= "statements" then return end
      for i = 1, #n - 1 do
         local s = n[i]
         if is_node(s) and s.kind == "if" then has_next[s] = true end
      end
   end)
   return has_next
end

local function ends_in_return(body)
   if type(body) ~= "table" or #body == 0 then return false end
   local last = body[#body]
   return is_node(last) and last.kind == "return"
end

local function lint_enum_exhaustive(ast, report, extra)
   extra = extra or {}
   local enums = collect_enums(ast)
   for name, set in pairs(extra.enums or {}) do
      if enums[name] == nil then enums[name] = set end
   end
   local has_next = mark_fallthrough(ast)
   walk(ast, function(n)
      if n.kind ~= "if" or not n.if_blocks then return end
      -- A single `if x == "a" then ... end` is a guard (early return / special case),
      -- not a dispatch over the enum: only chains with 2+ branches are checked.
      if #n.if_blocks < 2 then return end
      -- All branches return and code follows: the fallthrough is the `else`.
      if has_next[n] then
         local all_return = true
         for _, blk in ipairs(n.if_blocks) do
            if not ends_in_return(blk.body) then all_return = false break end
         end
         if all_return then return end
      end
      local subject, seen_lits, subject_node = nil, {}, nil
      for _, blk in ipairs(n.if_blocks) do
         if not blk.exp then return end -- has `else`: exhaustive by construction
         local s, lits, node = literal_tests(blk.exp)
         if not s then return end
         if subject and s ~= subject then return end
         subject = s
         subject_node = subject_node or node
         for _, v in ipairs(lits) do seen_lits[v] = true end
      end
      if not subject then return end

      local best_name, best_set
      -- Preferred: the checker's own answer for the subject's type.
      if extra.subject_enum and subject_node then
         local set, tname = extra.subject_enum(subject_node.y, subject_node.x, subject)
         if set == false then return end -- typed, not an enum: nothing to cover
         if set then best_name, best_set = tname, set end
      end
      -- Fallback (type unknown): the smallest known enum containing every literal tested.
      if not best_set then
         if next(enums) == nil then return end
         local best_size
         for name, set in pairs(enums) do
            local all, size = true, 0
            for _ in pairs(set) do size = size + 1 end
            for v in pairs(seen_lits) do
               if not set[v] then all = false break end
            end
            if all and (best_size == nil or size < best_size) then
               best_name, best_set, best_size = name, set, size
            end
         end
      end
      if not best_name then return end
      local missing = {}
      for v in pairs(best_set) do
         if not seen_lits[v] then missing[#missing + 1] = v end
      end
      if #missing > 0 then
         table.sort(missing)
         report("enum-exhaustive", n.y, n.x,
            "if-chain on '" .. subject .. "' does not cover enum " .. best_name .. " value(s): "
            .. table.concat(missing, ", ") .. "; add a branch or an else")
      end
   end)
end

---------------------------------------------------------------- shadow-local

local function lint_shadow(ast, report)
   local scopes = {}
   local function push() scopes[#scopes + 1] = {} end
   local function pop() scopes[#scopes] = nil end
   local function declare(name, y, x)
      if type(name) ~= "string" or name == "self" or name == "..." or name:sub(1, 1) == "_" then
         return
      end
      for i = #scopes - 1, 1, -1 do
         local outer = scopes[i][name]
         if outer then
            report("shadow-local", y, x,
               "local '" .. name .. "' shadows an outer local declared at line " .. outer)
            break
         end
      end
      scopes[#scopes][name] = y
   end
   local function declare_args(args)
      if type(args) ~= "table" then return end
      for _, a in ipairs(args) do
         if is_node(a) then declare(a.tk, a.y, a.x) end
      end
   end

   local visit
   local function visit_children(n)
      for k, v in pairs(n) do
         if not SKIP_KEYS[k] and type(v) == "table" then visit(v) end
      end
   end
   local seen = {}
   visit = function(n)
      if type(n) ~= "table" or seen[n] then return end
      seen[n] = true
      if not is_node(n) then
         visit_children(n)
         return
      end
      local k = n.kind
      if k == "statements" then
         push()
         for _, s in ipairs(n) do visit(s) end
         pop()
      elseif k == "local_declaration" then
         visit(n.exps)
         for _, v in ipairs(n.vars or {}) do
            if is_node(v) then declare(v.tk, v.y, v.x) end
         end
      elseif k == "local_function" then
         if is_node(n.name) then declare(n.name.tk, n.name.y, n.name.x) end
         push()
         declare_args(n.args)
         visit(n.body)
         pop()
      elseif k == "function" or k == "record_function" or k == "global_function" or k == "macroexp" or k == "local_macroexp" then
         push()
         declare_args(n.args)
         visit(n.body)
         pop()
      elseif k == "forin" then
         visit(n.exps)
         push()
         for _, v in ipairs(n.vars or {}) do
            if is_node(v) then declare(v.tk, v.y, v.x) end
         end
         visit(n.body)
         pop()
      elseif k == "fornum" then
         visit(n.from); visit(n.to); visit(n.step)
         push()
         if is_node(n.var) then declare(n.var.tk, n.var.y, n.var.x) end
         visit(n.body)
         pop()
      else
         visit_children(n)
      end
   end
   push()
   visit(ast)
   pop()
end

---------------------------------------------------------------- no-global

local GLOBAL_KINDS = { global_declaration = true, global_function = true, global_type = true }

local function lint_no_global(ast, report)
   walk(ast, function(n)
      if GLOBAL_KINDS[n.kind] then
         report("no-global", n.y, n.x, "global declaration; prefer a local and return it from the module")
      end
   end)
end

---------------------------------------------------------------- no-any

-- Walk everything including type tables; report each `any` type with a position.
local function lint_no_any(ast, report)
   local seen = {}
   local function go(t)
      if type(t) ~= "table" or seen[t] then return end
      seen[t] = true
      if t.typename == "any" and t.y then
         report("no-any", t.y, t.x, "explicit 'any' weakens type checking; use a concrete type or a record")
      end
      for k, v in pairs(t) do
         if k ~= "if_parent" and k ~= "type" and k ~= "expected" and type(v) == "table" then go(v) end
      end
   end
   go(ast)
end

---------------------------------------------------------------- explicit-number

-- Numeric literal used as an unannotated local's initializer: Teal infers `integer`
-- from `0` and `number` from `0.0`, and a later `total = total + w * h` then fails with
-- "got number, expected integer". Ask for the annotation up front.
local function numeric_literal(exp)
   if not is_node(exp) then return nil end
   if exp.kind == "integer" then return "integer" end
   if exp.kind == "number" then return "number" end
   -- unary minus on a literal (`-1`)
   if exp.kind == "op" and exp.op and exp.op.op == "-" and exp.e2 == nil then
      return numeric_literal(exp.e1)
   end
   return nil
end

local function lint_explicit_number(ast, report)
   walk(ast, function(n)
      if n.kind ~= "local_declaration" or not n.vars then return end
      -- `decltuple` is a tuple type; its member types sit in `.tuple` (one per annotated var).
      local decl = n.decltuple
      local annotated = (decl and decl.tuple and #decl.tuple) or 0
      for i, v in ipairs(n.vars) do
         if is_node(v) and i > annotated then
            local exp = n.exps and n.exps[i]
            local inferred = numeric_literal(exp)
            if inferred then
               local other = inferred == "integer" and "number" or "integer"
               report("explicit-number", v.y, v.x,
                  "'" .. tostring(v.tk) .. "' is inferred as " .. inferred .. " from its literal; write `local "
                  .. tostring(v.tk) .. ": " .. inferred .. " = ...` (or `: " .. other .. "`) to fix the numeric type explicitly")
            end
         end
      end
   end)
end

---------------------------------------------------------------- class-record

-- A record that declares metamethods (`metamethod __index: Actor`, `__call`, ...) is a
-- class in disguise: its behaviour lives in a metatable that `setmetatable` attaches at
-- run time. That metatable is not part of the value, so it is lost when the table is
-- serialized (`pairs` never sees it) or converted at the Rust boundary (`TealRecord`
-- copies fields). Opt-in inventory of such records for projects that keep boundary and
-- saved types as plain data.
local function record_meta_walk(t, name, node, report, seen, depth)
   if type(t) ~= "table" or seen[t] or depth > 12 then return end
   seen[t] = true
   if t.def then record_meta_walk(t.def, name, node, report, seen, depth + 1) return end
   if t.typename == "record" or t.typename == "interface" then
      local metas = {}
      for _, m in ipairs(t.meta_field_order or {}) do metas[#metas + 1] = m end
      if #metas == 0 and t.meta_fields then
         for m in pairs(t.meta_fields) do metas[#metas + 1] = m end
         table.sort(metas)
      end
      if #metas > 0 then
         report("class-record", t.y or node.y, t.x or node.x,
            "record " .. name .. " declares metamethod(s) " .. table.concat(metas, ", ")
            .. ": its behaviour lives in a metatable that serialization and the Rust boundary do not carry; "
            .. "keep it out of saved data and host signatures, or make it a plain data record")
      end
      for fname, ft in pairs(t.fields or {}) do
         record_meta_walk(ft, name .. "." .. fname, node, report, seen, depth + 1)
      end
   end
end

local function lint_class_record(ast, report)
   local seen = {}
   walk(ast, function(n)
      if (n.kind == "local_type" or n.kind == "global_type") and is_node(n.value) and n.value.newtype then
         record_meta_walk(n.value.newtype, n.var and n.var.tk or "?", n, report, seen, 0)
      end
   end)
end

---------------------------------------------------------------- entry

local RULES = {
   { "nil-index", lint_nil_index },
   { "enum-exhaustive", lint_enum_exhaustive },
   { "shadow-local", lint_shadow },
   { "no-global", lint_no_global },
   { "no-any", lint_no_any },
   { "explicit-number", lint_explicit_number },
   { "class-record", lint_class_record },
}

function L.rule_names()
   local out = {}
   for _, r in ipairs(RULES) do out[#out + 1] = r[1] end
   return out
end

-- Parse `+rule,-rule,...` on top of the defaults. Returns enabled set or nil, err.
function L.config(spec)
   local cfg = {}
   for k, v in pairs(L.DEFAULT) do cfg[k] = v end
   for item in (spec or ""):gmatch("[^,%s]+") do
      local sign, name = item:match("^([%+%-]?)([%w%-]+)$")
      if not name or cfg[name] == nil then
         return nil, "unknown lint rule: " .. tostring(item)
      end
      cfg[name] = (sign ~= "-")
   end
   return cfg
end

-- Returns list of { rule, y, x, msg } sorted by position, or nil, err on syntax error.
-- `extra` = { enums = name -> enumset, subject_enum = fn(y, x, key) } feeds
-- enum-exhaustive with what the checker resolved (see prelude.lua).
function L.run(src, filename, cfg, extra)
   cfg = cfg or L.DEFAULT
   local ast, errs = tl.parse(src, filename, "tl")
   if not ast or #errs > 0 then
      return nil, errs[1] and errs[1].msg or "parse failed"
   end
   local allows = collect_allows(src)
   local out = {}
   local function report(rule, y, x, msg)
      if allows[y] and allows[y][rule] then return end
      out[#out + 1] = { rule = rule, y = y or 0, x = x or 0, msg = msg .. " [htl " .. rule .. "]" }
   end
   for _, r in ipairs(RULES) do
      if cfg[r[1]] then r[2](ast, report, extra) end
   end
   table.sort(out, function(a, b)
      if a.y ~= b.y then return a.y < b.y end
      return a.x < b.x
   end)
   return out
end

return L
