-- htl lints: extra checks on the Teal syntax tree that Teal itself does not perform.
-- Lints run on a fresh `tl.parse` of the source (pure syntax, independent of the checker).
--
--   nil-index        `t[k].x` / `t[k]:m()` / `t[k]()` / `t[k][j]` -- indexing a map/array
--                    yields V, not V|nil in Teal, so chaining on it can raise at runtime.
--   sealed-record    a table constructor for a record marked `---@sealed`, or an `as` cast
--                    to one, outside the file that declares it (outside the functions the
--                    marker names, when it names any).
--   enum-exhaustive  `if e == "a" then ... elseif e == "b" then ... end` where the string
--                    literals belong to a declared enum: every value must be covered or an
--                    `else` branch must exist.
--   enum-cast        `e as E` where E is an enum and `e` is a string: the cast is erased,
--                    so the word enters the enum unchecked.
--   enum-table       a table constructor whose declared type maps an enum (`{string: E}`,
--                    `{E: T}`) and that leaves a variant out (or lists a word that is not
--                    one).
--   shadow-local     a local (or loop / parameter name) reuses the name of a local in an
--                    enclosing scope.
--   no-global        `global` declarations (prefer locals + module return).
--   no-any           explicit `any` in annotations or `as any` casts.   [allow: not said
--                    unless a project asks for it]
--   explicit-number  unannotated local initialized with a numeric literal (`local n = 0`
--                    infers integer, `0.0` infers number); ask for the annotation. [allow]
--   class-record     record declaring metamethods (a class): its metatable is not part of
--                    the value, so serialization and the Rust boundary drop it.   [allow]
--
-- Suppress per line with a trailing comment:  -- htl: allow(nil-index, shadow-local)

local tl = require("tl")
local L = {}

-- Which rules are on is not decided here. The registry — every rule name there is, its
-- default level, and which half of htl implements it — is `lint::RULES` in lint.rs, because
-- the project layer reports under those names too and could not read a list kept in Lua.
-- What this file owns is the twelve implementations below (`RULES`), and `L.run` is handed
-- the selection to run them under: a rule / on table, not a rule / level one. How much a
-- finding matters is read where the run is judged, so a rule moving between `warn` and
-- `deny` changes nothing about the work done here. `struct-fields` and `sealed-record` are on and still say
-- nothing until a record carries `---@struct` / `---@sealed`, which someone had to write.

local SKIP_KEYS = { if_parent = true, type = true, newtype = true, decltuple = true, expected = true }

local function is_node(t)
   return type(t) == "table" and type(t.kind) == "string"
end

-- Generic walk over syntax nodes (types are skipped), each table visited once.
-- The traversal itself is the expensive part (every field of every node table), so the
-- pre-order node list is computed once per root and replayed for every later walk of
-- the same root: seven rules walking an 850-line file cost one traversal, not seven.
local orders = setmetatable({}, { __mode = "k" }) -- root -> { node, node, ... }

local function traverse(root)
   local order, seen = {}, {}
   local function go(n)
      if type(n) ~= "table" or seen[n] then return end
      seen[n] = true
      if is_node(n) then order[#order + 1] = n end
      -- Statement lists first, in source order (lints that track "declared, then
      -- later assigned" depend on it); then the named children.
      for i = 1, #n do
         if type(n[i]) == "table" then go(n[i]) end
      end
      for k, v in pairs(n) do
         if type(k) ~= "number" and not SKIP_KEYS[k] and type(v) == "table" then go(v) end
      end
   end
   go(root)
   return order
end

local function walk(root, visit)
   local order = orders[root]
   if not order then
      order = traverse(root)
      orders[root] = order
   end
   for i = 1, #order do
      visit(order[i])
   end
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

-- The `-- htl: allow(a, b)` comments of a source, by line number.
--
-- A name may carry a `:` — that is how Teal's warning kinds are spelled (`tl:hint`) — so
-- the colon is part of a name here and never a separator. The sibling in lint.rs, which
-- answers the same question for the findings raised on the Rust side, splits on commas and
-- whitespace and so accepts the same shapes.
local function collect_allows(src)
   local allows = {}
   local y = 0
   for line in (src .. "\n"):gmatch("(.-)\n") do
      y = y + 1
      local names = line:match("%-%-%s*htl:%s*allow%(([%w%-:, ]+)%)")
      if names then
         allows[y] = allows[y] or {}
         for name in names:gmatch("[%w%-:]+") do allows[y][name] = true end
      end
   end
   return allows
end

-- Also read by the prelude, which answers the same question for the checker's own
-- warnings: they are named now, so a line may allow one by name like any other finding.
L.collect_allows = collect_allows

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

---------------------------------------------------------------- enum-cast

-- A Teal enum is a string at run time and `as` is erased with the types, so `e as E` is
-- the one place a word enters the enum with nothing looking at it. The two sites where
-- that happens are the two that matter: a row read back from a store, and a word a person
-- typed. With `"opne"` in the store every `== "open"` is false, the value falls out of
-- every branch, and nothing raises -- `enum-exhaustive` guards the `if` chain, it cannot
-- see that the value never entered the set.
--
-- Only a cast the checker types as `string` is reported. A value it already types as the
-- enum (or a union holding it) is a cast that restates what is known, and a string
-- *literal* is checked by the literal itself: `"open" as E` fails the check if "open" is
-- not a value of E.
local function unparen(n)
   while is_node(n) and n.kind == "paren" do n = n.e1 end
   return n
end

-- The cast's type as it is written at the site (`defs.State`), for the message: the
-- checker's own name for it is the bare `State`, which is not what the line says.
local function cast_written(n)
   local ct = is_node(n.e2) and n.e2.casttype
   if type(ct) ~= "table" or type(ct.names) ~= "table" or #ct.names == 0 then return nil end
   return table.concat(ct.names, ".")
end

local function lint_enum_cast(ast, report, extra)
   local cast_at = extra and extra.cast_at
   if not cast_at then return end
   walk(ast, function(n)
      if n.kind ~= "op" or not n.op or n.op.op ~= "as" then return end
      local from = unparen(n.e1)
      if not is_node(from) or from.kind == "string" then return end
      local target, from_type = cast_at(n.y, n.x, from.y, from.x)
      if not target or from_type ~= "string" then return end
      local written = cast_written(n) or target.name
      report("enum-cast", n.y, n.x,
         "`as " .. written .. "` is not checked at run time; look the word up in a table typed {string: "
         .. written .. "}, or declare the enum on the host")
   end)
end

---------------------------------------------------------------- enum-table

-- The hand-written answer to `enum-cast` is a lookup table: `{ open = "open", ... }` typed
-- `{string: E}`, read as `states[s] or <default>`, which is total where the cast was not.
-- The table is then on its own -- add a value to the enum and it is one entry short, and
-- `enum-exhaustive` checks `if` chains, not constructors. This rule is that check, and
-- `htl fix enum-table` fills the entry in, which is the `enum-exhaustive` story for tables.
--
-- The words a constructor lists are its keys, in both shapes the enum can be mapped by:
-- `{string: E}` (the lookup above) and `{E: T}` (a table of one thing per value). An array
-- of the enum is not looked at: `{E}` is a selection, not a mapping — a list of the styles
-- one branch uses, the behaviours one test walks — and asking it for every value is noise
-- [measured on a 22k-line dogfood project: 13 array literals, 13 of them a selection].
--
-- Two constructors are left alone as well: an empty one (which says nothing about the
-- words, and is how a table that is filled later is written), and one whose keys are not
-- all literals (a computed key leaves the word set unknown). A table built by a call has
-- no constructor here at all, which is the exemption `enum-exhaustive` makes too.
local LUA_KEYWORDS = {
   ["and"] = true, ["break"] = true, ["do"] = true, ["else"] = true, ["elseif"] = true,
   ["end"] = true, ["false"] = true, ["for"] = true, ["function"] = true, ["goto"] = true,
   ["if"] = true, ["in"] = true, ["local"] = true, ["nil"] = true, ["not"] = true,
   ["or"] = true, ["repeat"] = true, ["return"] = true, ["then"] = true, ["true"] = true,
   ["until"] = true, ["while"] = true,
}

-- `open`, or `["end"]` for a word a bare key cannot spell.
local function entry_key(name)
   if name:match("^[%a_][%w_]*$") and not LUA_KEYWORDS[name] then return name end
   return "[" .. string.format("%q", name) .. "]"
end

-- `open = "open"`, or `["end"] = "end"` for a value that is not a bare key.
local function entry_text(name)
   return entry_key(name) .. " = " .. string.format("%q", name)
end

-- The line with its trailing comment removed. Quotes are tracked so that a `--` inside a
-- string is not mistaken for the start of one, which is what decides where a comma goes.
local function strip_comment(s)
   local i, q = 1, nil
   while i <= #s do
      local c = s:sub(i, i)
      if q then
         if c == "\\" then i = i + 1
         elseif c == q then q = nil end
      elseif c == '"' or c == "'" then
         q = c
      elseif c == "-" and s:sub(i + 1, i + 1) == "-" then
         return s:sub(1, i - 1)
      end
      i = i + 1
   end
   return s
end

-- Edits that add an entry for each of `names` to the constructor `n`. The layout comes
-- from the source, not the tree: where the last entry ends, whether it ends in a comma,
-- and how far it is indented are all things only the lines know.
--
-- `opts.entry(name)` writes one entry and defaults to the identity mapping `enum-table`
-- fills a lookup in with; `opts.applicability` classes the fix; `opts.split` gives every
-- name an edit of its own, so an editor can offer one code action per name rather than
-- one for the lot.
local function table_entry_fix(lines, n, names, opts)
   opts = opts or {}
   local entry = opts.entry or entry_text
   local applicability = opts.applicability or "safe"
   if not lines or not n.yend or not n.xend then return nil end
   local last_y, last_x
   for y = n.yend, n.y, -1 do
      local line = lines[y]
      if not line then return nil end
      local from = (y == n.y) and (n.x + 1) or 1
      local to = (y == n.yend) and (n.xend - 1) or #line
      if to >= from then
         local seg = strip_comment(line:sub(from, to))
         local col = seg:match("^.*()%S")
         if col then
            last_y, last_x = y, from + col - 1
            break
         end
      end
   end
   -- An empty constructor has no entry to read a layout off. On one line the entries go
   -- between the braces, spaced the way `{ a = 1 }` is written; written open across lines
   -- there is nothing to copy and no fix.
   if not last_y then
      if n.y ~= n.yend or not opts.split then return nil end
      local col = n.x + 1
      local pad = lines[n.yend]:sub(n.xend - 1, n.xend - 1):match("%s") and "" or " "
      local edits = {}
      for i, name in ipairs(names) do
         edits[#edits + 1] = {
            line = n.y, col = col, end_line = n.y, end_col = col,
            text = (i == 1 and " " or ", ") .. entry(name) .. (i == #names and pad or ""),
         }
      end
      return { applicability = applicability, edits = edits }
   end
   local comma = lines[last_y]:sub(last_x, last_x) == "," and "" or ","
   -- The closing brace on the last entry's own line: everything stays on that line.
   if last_y == n.yend then
      local col = last_x + 1
      local edits = {}
      if opts.split then
         for i, name in ipairs(names) do
            edits[#edits + 1] = {
               line = last_y, col = col, end_line = last_y, end_col = col,
               text = (i == 1 and comma or ",") .. " " .. entry(name),
            }
         end
      else
         local parts = {}
         for _, name in ipairs(names) do parts[#parts + 1] = entry(name) end
         edits[1] = {
            line = last_y, col = col, end_line = last_y, end_col = col,
            text = comma .. " " .. table.concat(parts, ", "),
         }
      end
      return { applicability = applicability, edits = edits }
   end
   local indent = lines[last_y]:match("^(%s*)") or ""
   -- Insert before the brace, or before its indentation when it sits on its own line, so
   -- that the brace keeps the column it had.
   local before = lines[n.yend]:sub(1, n.xend - 1)
   local col = before:match("^%s*$") and 1 or n.xend
   local edits = {}
   if comma ~= "" then
      edits[#edits + 1] = {
         line = last_y, col = last_x + 1, end_line = last_y, end_col = last_x + 1, text = comma,
      }
   end
   local text = {}
   for _, name in ipairs(names) do
      local line = indent .. entry(name) .. ",\n"
      if opts.split then
         edits[#edits + 1] = {
            line = n.yend, col = col, end_line = n.yend, end_col = col, text = line,
         }
      else
         text[#text + 1] = line
      end
   end
   if #text > 0 then
      edits[#edits + 1] = {
         line = n.yend, col = col, end_line = n.yend, end_col = col, text = table.concat(text),
      }
   end
   return { applicability = applicability, edits = edits }
end

-- The keys the constructor lists, in source order, or nil when one of them is not a
-- literal. For `{E: T}` these are the enum's own spellings; for `{string: E}` they are the
-- words the program will look up, which is the same set when the table is that lookup.
local function listed_words(n)
   local words, order = {}, {}
   for _, item in ipairs(n) do
      if type(item) ~= "table" then return nil end
      local w
      if is_node(item.key) then
         if item.key.kind == "string" then
            w = unquote(item.key.tk or "")
         elseif item.key.kind == "identifier" then
            w = item.key.tk
         end
      end
      if not w then return nil end
      if not words[w] then
         words[w] = true
         order[#order + 1] = w
      end
   end
   return words, order
end

local function lint_enum_table(ast, report, extra)
   local enum_table_at = extra and extra.enum_table_at
   if not enum_table_at then return end
   walk(ast, function(n)
      if n.kind ~= "literal_table" or not n.y or not n.x then return end
      if #n == 0 then return end
      local spec = enum_table_at(n.y, n.x)
      if not spec then return end
      -- `{E: T}` first: when both ends are enums the keys are still the words listed.
      local target, position = spec.key, "key"
      if not target then target, position = spec.value, "value" end
      if not target then return end
      local words, order = listed_words(n)
      if not words then return end

      local declared = {}
      for _, v in ipairs(target.values) do declared[v] = true end
      local unknown, covered = {}, 0
      for _, w in ipairs(order) do
         if declared[w] then covered = covered + 1 else unknown[#unknown + 1] = w end
      end
      local missing = {}
      for _, v in ipairs(target.values) do
         if not words[v] then missing[#missing + 1] = v end
      end
      -- `{string: E}`: the enum is what the table maps *to*, so the keys are the enum's
      -- own spellings only when the table is that lookup. One key that is a value says it
      -- is; none says this is some other map, and none of this rule's business.
      if position == "value" and covered == 0 then return end
      if #missing == 0 and #unknown == 0 then return end
      table.sort(unknown)

      local parts = {}
      if #missing > 0 then
         parts[#parts + 1] = "does not list " .. target.name .. " value(s): " .. table.concat(missing, ", ")
      end
      if #unknown > 0 then
         parts[#parts + 1] = "lists word(s) that are not " .. target.name .. " value(s): "
            .. table.concat(unknown, ", ")
      end
      local msg = "table typed " .. spec.name .. " " .. table.concat(parts, ", and ")
      local fix
      if #missing > 0 then
         if position == "value" then
            msg = msg .. "; a word it does not list looks up as nil"
            fix = table_entry_fix(extra.lines, n, missing)
         else
            msg = msg .. "; add the entry (what it maps to is not something a fix can invent)"
         end
      end
      report("enum-table", n.y, n.x, msg, fix)
   end)
end

---------------------------------------------------------------- union-exhaustive

-- `x is A` — the subject variable and the type named, or nil for any other test.
local function is_test(exp)
   if not is_node(exp) or exp.kind ~= "op" or not exp.op or exp.op.op ~= "is" then
      return nil
   end
   local subject = exp.e1
   if not is_node(subject) or subject.kind ~= "variable" or not subject.tk then return nil end
   local target = exp.e2
   if not is_node(target) then return nil end
   local name = target.casttype and target.casttype.names
      and target.casttype.names[#target.casttype.names]
   name = name or target.tk
   if not name then return nil end
   return subject.tk, name, subject
end

-- An `is` chain over a union that leaves a variant untested and has no `else`. The
-- sibling of `enum-exhaustive`, and it exists for the same reason: adding a variant
-- otherwise leaves every chain that predates it compiling, with the new one falling
-- wherever the last branch happened to lead.
--
-- The union's members come from the checker (`extra.union_at`), not from the tests: a
-- chain that names two variants tells you nothing about how many there are.
local function lint_union_exhaustive(ast, report, extra)
   extra = extra or {}
   local union_at = extra.union_at
   if not union_at then return end
   local has_next = mark_fallthrough(ast)
   walk(ast, function(n)
      if n.kind ~= "if" or not n.if_blocks then return end
      -- One `if x is A then ... end` is a guard, not a dispatch (same rule as enums).
      if #n.if_blocks < 2 then return end
      if has_next[n] then
         local all_return = true
         for _, blk in ipairs(n.if_blocks) do
            if not ends_in_return(blk.body) then all_return = false break end
         end
         if all_return then return end
      end
      local subject, tested, first = nil, {}, nil
      for _, blk in ipairs(n.if_blocks) do
         if not blk.exp then return end -- has `else`: exhaustive by construction
         local s, name, node = is_test(blk.exp)
         if not s then return end
         if subject and s ~= subject then return end
         subject = s
         first = first or node
         tested[name] = true
      end
      if not first then return end
      local members = union_at(first.y, first.x)
      if not members then return end -- unknown, or typed and not a union
      local missing = {}
      for name in pairs(members) do
         if not tested[name] then missing[#missing + 1] = name end
      end
      if #missing == 0 then return end
      table.sort(missing)
      report("union-exhaustive", n.y, n.x,
         "if-chain on '" .. subject .. "' does not cover " .. table.concat(missing, ", ")
         .. "; add a branch or an else")
   end)
end

---------------------------------------------------------------- shadow-local

local function lint_shadow(ast, report)
   local scopes = {}
   local function push() scopes[#scopes + 1] = {} end
   local function pop() scopes[#scopes] = nil end
   -- `module` = the `require("...")` name the outer local was bound to, if any. In a
   -- growing codebase the typical hit is a new parameter or local taking the name of a
   -- module required at the top of the file; say so instead of pointing at a line.
   local function declare(name, y, x, module)
      if type(name) ~= "string" or name == "self" or name == "..." or name:sub(1, 1) == "_" then
         return
      end
      for i = #scopes - 1, 1, -1 do
         local outer = scopes[i][name]
         if outer then
            if outer.module then
               report("shadow-local", y, x,
                  "local '" .. name .. "' shadows the module '" .. outer.module .. "' required at line "
                  .. outer.y .. "; inside this scope the module is unreachable, rename the local")
            else
               report("shadow-local", y, x,
                  "local '" .. name .. "' shadows an outer local declared at line " .. outer.y)
            end
            break
         end
      end
      scopes[#scopes][name] = { y = y, module = module }
   end
   -- `require("<name>")` in expression position: the module name, else nil.
   local function required_module(exp)
      if is_node(exp) and exp.kind == "op" and exp.op and exp.op.op == "@funcall"
         and is_node(exp.e1) and exp.e1.kind == "variable" and exp.e1.tk == "require"
         and type(exp.e2) == "table" and is_node(exp.e2[1]) and exp.e2[1].kind == "string" then
         return unquote(exp.e2[1].tk)
      end
      return nil
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
         for i, v in ipairs(n.vars or {}) do
            if is_node(v) then declare(v.tk, v.y, v.x, required_module(n.exps and n.exps[i])) end
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

local function lint_no_global(ast, report, extra)
   local lines = extra and extra.lines
   walk(ast, function(n)
      if GLOBAL_KINDS[n.kind] then
         -- `global` -> `local` at the keyword. The node's position is the declared
         -- name; the keyword is the last `global` on the line before it. Unsafe: the
         -- name stops being visible to other chunks, which may be what some file relied on.
         local fix
         local line = lines and lines[n.y]
         local kw = line and line:sub(1, (n.x or 1) - 1):match(".*()global")
         if kw then
            fix = {
               applicability = "unsafe",
               edits = { { line = n.y, col = kw, end_line = n.y, end_col = kw + 6, text = "local" } },
            }
         end
         report("no-global", n.y, n.x, "global declaration; prefer a local and return it from the module", fix)
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

-- Does the expression carry a non-integer: a float literal, `/` or `^` (both yield
-- number in Teal even on integers)?
local function mixes_number(exp)
   local hit = false
   walk(exp, function(n)
      if n.kind == "number" then hit = true end
      if n.kind == "op" and n.op and (n.op.op == "/" or n.op.op == "^") then hit = true end
   end)
   return hit
end

-- Only locals that are *later* assigned a number expression are reported: a plain
-- integer counter (`local i = 1`, `local MASK = 0xff`) never meets a number and needs
-- no annotation (dogfood: 37 of 37 hits in a 5k-line game were counters).
local function lint_explicit_number(ast, report)
   local ints = {} -- name -> declaring var node (later declaration shadows)
   walk(ast, function(n)
      if n.kind == "local_declaration" and n.vars then
         -- `decltuple` is a tuple type; its member types sit in `.tuple` (one per annotated var).
         local decl = n.decltuple
         local annotated = (decl and decl.tuple and #decl.tuple) or 0
         for i, v in ipairs(n.vars) do
            if is_node(v) then
               if i > annotated and numeric_literal(n.exps and n.exps[i]) == "integer" then
                  ints[v.tk] = v
               else
                  ints[v.tk] = nil
               end
            end
         end
      elseif n.kind == "assignment" and n.vars then
         for i, v in ipairs(n.vars) do
            if is_node(v) and v.kind == "variable" and ints[v.tk] and n.exps and n.exps[i]
               and mixes_number(n.exps[i]) then
               local d = ints[v.tk]
               ints[v.tk] = nil
               -- Safe fix: `: number` right after the name. The local already receives
               -- a number, so the annotation states what the code does.
               local after = d.x + #tostring(v.tk)
               report("explicit-number", d.y, d.x,
                  "'" .. tostring(v.tk) .. "' is inferred as integer from its literal but is assigned a number at line "
                  .. tostring(n.y) .. "; write `local " .. tostring(v.tk) .. ": number = ...` to fix the numeric type explicitly", {
                  applicability = "safe",
                  edits = { { line = d.y, col = after, end_line = d.y, end_col = after, text = ": number" } },
               })
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

---------------------------------------------------------------- struct-fields

-- A record marked `---@struct` is built whole: every field it declares is present at
-- every construction site, except the ones marked `---@optional`. Teal has no `?` for
-- record fields, so without this a record the program builds itself reads as if any field
-- might be absent, and every use site pays for that with a nil check.
--
-- Which record a bare `{ ... }` is being built as is type information, and this rule is
-- run over a syntax-only parse (see L.run). `extra.struct_at(y, x)` answers it from the
-- checker's position report; the rule only compares key sets.
-- Edit distance, stopped as soon as it is past `bound`: the answer here is only ever
-- "close enough or not", and a full distance between two unrelated names is wasted work.
--
-- A transposition costs one, not two (optimal string alignment). `lable` for `label` is
-- among the commonest ways to mistype a name, and plain Levenshtein charges it two, which
-- puts it outside the bound for exactly the short names most fields have. Widening the
-- bound instead would let in names that merely share letters; this admits the one mistake
-- that was missing and nothing else.
local function within(a, b, bound)
   if a == b then return true end
   if math.abs(#a - #b) > bound then return false end
   local prev2 = nil
   local prev = {}
   for j = 0, #b do prev[j] = j end
   for i = 1, #a do
      local cur = { [0] = i }
      local best = cur[0]
      local ca = a:byte(i)
      for j = 1, #b do
         local cost = (ca == b:byte(j)) and 0 or 1
         local d = math.min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + cost)
         if i > 1 and j > 1 and ca == b:byte(j - 1) and a:byte(i - 1) == b:byte(j) then
            d = math.min(d, prev2[j - 2] + 1)
         end
         cur[j] = d
         if d < best then best = d end
      end
      if best > bound then return false end
      prev2 = prev
      prev = cur
   end
   return prev[#b] <= bound
end

-- How far apart two names may be and still be the same intention. One edit is a typo in
-- anything; a second is only believable once the name is long enough that two edits are
-- still a small share of it.
local function near_bound(name)
   return #name >= 8 and 2 or 1
end

-- What a missing field is given by the fix: not a value at all. A record field has a type
-- and no honest zero -- `hp: integer` is not 0 and `name: string` is not "" -- and
-- `hp = nil` type-checks, since every Teal record field is nilable, which is the whole
-- reason `---@struct` exists. A fix that filled the gap would satisfy the lint, pass the
-- checker and ship the wrong value silently; this one has to be replaced, and says so by
-- being refused where it stands.
--
-- The form is a call to a name the project does not have, carrying the type the field is
-- declared with: `hp = htl_fixme("integer")`. The type is there to be read, and the call
-- is what refuses -- `unknown variable: htl_fixme`, wherever it lands.
--
-- The type spelled bare (`hp = integer`) was the other candidate and does not hold. Two
-- of the three kinds of type pass the checker that way [measured on this tl]: a record's
-- name in value position is the type's own table, and assigning it to a field of that
-- record is accepted (`child = Child` reports nothing), as is `x = any` on a field typed
-- `any`. Only the primitives refuse. A type that is not a name at all (`{string}`,
-- `function(integer): string`, a union) would not even parse where a value goes, and a
-- suggestion that turns the file into a syntax error is worse than one that says what is
-- wanted. Quoted inside a call, every type reads the same way and none of them checks.
local function field_placeholder(ty)
   if not ty or ty == "" then return "htl_fixme()" end
   return "htl_fixme(" .. string.format("%q", ty) .. ")"
end

local function lint_struct_fields(ast, report, extra)
   local struct_at = extra and extra.struct_at
   if not struct_at then return end
   walk(ast, function(n)
      if n.kind ~= "literal_table" or not n.y or not n.x then return end
      local spec = struct_at(n.y, n.x)
      if not spec then return end
      local present = {}
      for _, item in ipairs(n) do
         if type(item) == "table" and item.key then
            if item.key.kind == "string" then
               present[unquote(item.key.tk or "")] = true
            elseif item.key.kind == "identifier" then
               present[item.key.tk] = true
            end
         end
      end
      local missing = {}
      for name in pairs(spec.required) do
         if not present[name] then missing[#missing + 1] = name end
      end
      if #missing == 0 then return end
      table.sort(missing)

      -- Keys the literal sets that the record does not declare. One of them being a near
      -- miss for a missing field is a misspelling, and saying so beats the standing
      -- advice: "mark it ---@optional" is the wrong fix for a typo.
      local stray = {}
      for name in pairs(present) do
         if not (spec.declared or {})[name] then stray[#stray + 1] = name end
      end
      table.sort(stray)
      local hints, used = {}, {}
      for _, want in ipairs(missing) do
         for _, got in ipairs(stray) do
            if not used[got] and within(want, got, near_bound(want)) then
               used[got] = true
               hints[#hints + 1] = { want = want, got = got }
               break
            end
         end
      end

      local tail = " (set it here, or mark the field ---@optional where it is declared)"
      if #hints == 1 and #missing == 1 then
         tail = (" (the literal sets `%s`)"):format(hints[1].got)
      elseif #hints > 0 then
         local parts = {}
         for _, h in ipairs(hints) do
            parts[#parts + 1] = ("`%s` for %s"):format(h.got, h.want)
         end
         tail = (" (the literal sets %s)"):format(table.concat(parts, ", "))
      end

      -- The fix spells the missing fields at the site, in the order the record declares
      -- them, for the author to put values on -- one edit each. A field the message
      -- already blames on a misspelling is left out of it: the answer there is to correct
      -- the key that is there, not to add a second one beside it.
      local misspelled = {}
      for _, h in ipairs(hints) do misspelled[h.want] = true end
      local names, ty = {}, {}
      for _, f in ipairs(spec.fields or {}) do
         if not present[f.name] and not misspelled[f.name] then
            names[#names + 1] = f.name
            ty[f.name] = f.type
         end
      end
      local fix
      if #names > 0 then
         fix = table_entry_fix(extra.lines, n, names, {
            applicability = "suggest",
            split = true,
            entry = function(name)
               return entry_key(name) .. " = " .. field_placeholder(ty[name])
            end,
         })
      end

      report(
         "struct-fields",
         n.y,
         n.x,
         spec.name .. " is built without " .. table.concat(missing, ", ") .. tail,
         fix
      )
   end)
end

---------------------------------------------------------------- sealed-record

-- A record marked `---@sealed` is built where it is declared and nowhere else. Some
-- records mean "this went through the check" -- a `Judged` only `gate.judge` is supposed
-- to produce, a state only a transition may mint -- and Teal has no private field and no
-- sealed constructor to say it with: `{ ... }` with the right keys builds one anywhere,
-- and `as` gets past even a mismatch because it is erased. The marker says it and this
-- rule holds the boundary.
--
-- Which record a `{ ... }` is built as, and which one an `as` lands on, is type
-- information; `extra.sealed_at(y, x)` answers both from the checker's position report
-- (see prelude.lua). What this rule adds is the site: the file it is in, and the function.
--
-- `---@sealed(gate.judge)` narrows the boundary from the file to those functions, inside
-- the declaring file. A function matches on the name as written (`gate.judge`) or on its
-- last segment (`judge`), because the local the site spells the module with need not be
-- the name the marker uses. A function that is assigned rather than declared
-- (`gate.judge = function() ... end`) has no name of its own here, and the site counts as
-- being in the enclosing function -- as a callback written inside `gate.judge` does.
local FUNCTION_KINDS = {
   ["function"] = true, ["local_function"] = true, ["global_function"] = true,
   ["record_function"] = true, ["macroexp"] = true, ["local_macroexp"] = true,
}

-- `function gate.judge()` -> "gate.judge", `function Gate:judge()` -> "Gate:judge",
-- `local function judge()` -> "judge", an anonymous function -> nil.
local function fn_written_name(n)
   if not is_node(n.name) or not n.name.tk then return nil end
   local owners = {}
   local owner = n.fn_owner
   while is_node(owner) do
      if owner.kind == "op" and owner.op and owner.op.op == "." and is_node(owner.e2) then
         table.insert(owners, 1, owner.e2.tk or "?")
         owner = owner.e1
      else
         table.insert(owners, 1, owner.tk or "?")
         break
      end
   end
   if #owners == 0 then return n.name.tk end
   return table.concat(owners, ".") .. (n.is_method and ":" or ".") .. n.name.tk
end

local function fn_allowed(fns, name)
   if not name then return false end
   local last = name:match("([^.:]+)$") or name
   for _, want in ipairs(fns) do
      if want == name or (want:match("([^.:]+)$") or want) == last then return true end
   end
   return false
end

local function lint_sealed_record(ast, report, extra)
   local sealed_at = extra and extra.sealed_at
   if not sealed_at then return end
   local function allowed(spec, fn)
      if spec.fns then return spec.here and fn_allowed(spec.fns, fn) end
      return spec.here
   end
   -- The record is named as its declaration names it, at both kinds of site: a cast writes
   -- the type out and a constructor does not, and one name for the rule reads better than
   -- one name per site.
   local function complain(spec, n)
      local msg = "`" .. spec.name .. "` is sealed: built only in " .. spec.file
      if spec.fns then msg = msg .. " by " .. table.concat(spec.fns, " or ") end
      report("sealed-record", n.y, n.x, msg)
   end
   local seen = {}
   local function visit(n, fn)
      if type(n) ~= "table" or seen[n] then return end
      seen[n] = true
      if is_node(n) then
         if FUNCTION_KINDS[n.kind] then fn = fn_written_name(n) or fn end
         if n.kind == "literal_table" and n.y and n.x then
            local spec = sealed_at(n.y, n.x)
            if spec and not allowed(spec, fn) then complain(spec, n) end
         elseif n.kind == "op" and n.op and n.op.op == "as" and n.y and n.x then
            local spec = sealed_at(n.y, n.x)
            if spec and not allowed(spec, fn) then complain(spec, n) end
         end
      end
      for k, v in pairs(n) do
         if not SKIP_KEYS[k] and type(v) == "table" then visit(v, fn) end
      end
   end
   visit(ast, nil)
end

local RULES = {
   { "nil-index", lint_nil_index },
   { "struct-fields", lint_struct_fields },
   { "sealed-record", lint_sealed_record },
   { "enum-exhaustive", lint_enum_exhaustive },
   { "enum-cast", lint_enum_cast },
   { "enum-table", lint_enum_table },
   { "union-exhaustive", lint_union_exhaustive },
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

-- Returns list of { rule, y, x, msg } sorted by position, or nil, err on syntax error.
-- `extra` = { enums = name -> enumset, subject_enum = fn(y, x, key) } feeds
-- enum-exhaustive with what the checker resolved (see prelude.lua).
-- `cfg` is rule -> on, as the Rust side resolved it (`Htl::select_lints`). Nothing is a
-- rule this file decides for itself, so a missing table runs nothing.
function L.run(src, filename, cfg, extra)
   cfg = cfg or {}
   -- Always a fresh parse. The checker's AST looks like a free second copy, but the
   -- checker hangs resolved types off nodes, and through them the declarations of
   -- *other* files become reachable: `no-any` then reported `any`s from test.d.tl at
   -- their own positions in every file that required it [measured on a 39-file
   -- project]. SKIP_KEYS cannot enumerate every such edge; a syntax-only tree can.
   local ast, errs = tl.parse(src, filename, "tl")
   if not ast or #errs > 0 then
      return nil, errs[1] and errs[1].msg or "parse failed"
   end
   local allows = collect_allows(src)
   -- Source lines by number, for rules whose fix needs a keyword the AST does not
   -- position (`global`).
   extra = extra or {}
   if not extra.lines then
      local lines = {}
      for l in (src .. "\n"):gmatch("([^\n]*)\n") do lines[#lines + 1] = l end
      extra.lines = lines
   end
   local out = {}
   -- `fix` (optional) = { applicability = "safe" | "unsafe" | "suggest", edits = { { line,
   -- col, end_line, end_col, text } } }: a mechanical rewrite `htl fix` may apply.
   -- Positions are 1-based line / byte column; an insertion has end == start.
   local function report(rule, y, x, msg, fix)
      if allows[y] and allows[y][rule] then return end
      out[#out + 1] = { rule = rule, y = y or 0, x = x or 0, msg = msg .. " [htl " .. rule .. "]", fix = fix }
   end
   local profile = os.getenv("HTL_PROFILE") ~= nil
   for _, r in ipairs(RULES) do
      if cfg[r[1]] then
         local t0 = os.clock()
         r[2](ast, report, extra)
         if profile then
            io.stderr:write(string.format("profile:   rule %-16s %7.1f ms  %s\n", r[1], (os.clock() - t0) * 1000, filename))
         end
      end
   end
   table.sort(out, function(a, b)
      if a.y ~= b.y then return a.y < b.y end
      return a.x < b.x
   end)
   return out
end

return L
