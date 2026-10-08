-- htl fmt: whitespace formatter for Teal (gofmt-lite).
--
-- What it normalizes:
--   * indentation, recomputed from the syntax tree (block bodies, table constructors,
--     call argument lists, record/enum/interface bodies incl. nested type decls)
--   * one extra level for continuation lines (previous line ends with an operator / `=`,
--     or this line starts with a binary operator)
--   * trailing whitespace, runs of blank lines (max 2), leading/trailing blank lines,
--     final newline
-- What it leaves alone: token spacing inside a line, line breaks, and every line that
-- starts inside a long string / long comment.

local tl = require("tl")
local F = {}

---------------------------------------------------------------- source scanning

-- Set of line numbers whose *start* is inside a multi-line string or comment.
local function protected_lines(src)
   local prot = {}
   local i, n, y = 1, #src, 1
   local state = nil -- nil | { kind = "long", eq = N } | { kind = "short", q = '"' }
   while i <= n do
      local c = src:sub(i, i)
      if c == "\n" then
         y = y + 1
         if state then prot[y] = true end
         i = i + 1
      elseif state then
         if state.kind == "long" then
            local close = "]" .. string.rep("=", state.eq) .. "]"
            if src:sub(i, i + #close - 1) == close then
               state = nil
               i = i + #close
            else
               i = i + 1
            end
         else
            if c == "\\" then
               if src:sub(i + 1, i + 1) == "\n" then
                  y = y + 1
                  prot[y] = true
               end
               i = i + 2
            elseif c == state.q then
               state = nil
               i = i + 1
            else
               i = i + 1
            end
         end
      else
         if src:sub(i, i + 1) == "--" then
            local eq = src:match("^%-%-%[(=*)%[", i)
            if eq then
               state = { kind = "long", eq = #eq }
               i = i + 4 + #eq
            else
               local nl = src:find("\n", i, true)
               i = nl or (n + 1)
            end
         elseif c == "[" then
            local eq = src:match("^%[(=*)%[", i)
            if eq then
               state = { kind = "long", eq = #eq }
               i = i + 2 + #eq
            else
               i = i + 1
            end
         elseif c == '"' or c == "'" then
            state = { kind = "short", q = c }
            i = i + 1
         else
            i = i + 1
         end
      end
   end
   return prot
end

local function split_lines(src)
   local lines = {}
   for line in (src .. "\n"):gmatch("(.-)\n") do
      lines[#lines + 1] = (line:gsub("\r$", ""))
   end
   -- the trailing "\n" we appended produces one extra empty line when src ends with "\n"
   if src:sub(-1) == "\n" then lines[#lines] = nil end
   return lines
end

---------------------------------------------------------------- spans from the syntax tree

local SKIP_KEYS = { if_parent = true, type = true, newtype = true, decltuple = true, expected = true }

local function is_node(t)
   return type(t) == "table" and type(t.kind) == "string"
end

-- Index of the token at a position, so a node can be asked what came just before it.
local function tokens_by_pos(tokens)
   local at = {}
   for i, t in ipairs(tokens) do
      if t.y and t.x then
         at[t.y] = at[t.y] or {}
         at[t.y][t.x] = i
      end
   end
   return at
end

-- The line a block's body opens on: the line of the token just before the body's first
-- one, which is whatever introduced it (`then`, `else`, `do`, `repeat`, a function's
-- return type). Not the body's own line, because `statements` starts at its first
-- *statement* — a body whose first line is a comment starts below it, and a body with no
-- statements at all starts at the terminator that ends it. Either way the lines between
-- belong to the block, and asking the token stream is what says so; comments are attached
-- to the tokens around them rather than being tokens (`tl.lex`), so the one before is
-- always the opener.
--
-- Takes the position rather than a node, so a caller can probe a position no node sits
-- at (the keyword's, see `collect_spans` below) the same way.
local function opener_pos(at, tokens, y, x)
   local i = at[y] and at[y][x]
   if i and i > 1 then
      local prev = tokens[i - 1]
      if prev and prev.y and prev.x then return prev.y, prev.x end
   end
   return y, x
end

-- `await <call>` / `async local` put the keyword's own position on the node they
-- rewrite (`htl_await_at` / `htl_async_at`, prelude.lua's `apply_async_marks`) — not
-- `async function`, which only sets `htl_async = true` with no position of its own.
-- Two things in here read that position: a block whose first statement is one of these
-- marked nodes finds the block's opener (`then` / `do` / ...) from the keyword's
-- position rather than the node's own, which the token stream has nothing at once the
-- node's `x` has been moved to the keyword's column; and the line(s) the marked node
-- goes on to occupy are a continuation of a line that held nothing but the keyword, the
-- same as a line ending in `+` is a continuation of an arithmetic expression, except no
-- operator marks it — the mark is this position instead.
local function keyword_mark(n)
   return n.htl_await_at or n.htl_async_at
end

-- The last source line any part of `n` reaches: the biggest `y` / `yend` found anywhere
-- in its subtree. Needed because neither node this file reads `htl_await_at` /
-- `htl_async_at` off has its own `yend` -- a `@funcall` op node's `y, x` is its argument
-- list's (vendor/tl.lua's parser builds it that way) and carries no `yend` of its own,
-- and `local_declaration` likewise leaves the line its assignment ends on to the last
-- expression in `exps` -- so the one place that line is on record is wherever the
-- parser *did* call `end_at` lower in the tree: the argument list's closing `)`, the
-- closing `}` of a table, the `end` of a function literal passed as an argument, and so
-- on, however deep the awaited call or the task's expression nests them. Deduped like
-- `collect_spans`'s own walk, and for the same reason (`if_parent` and friends make the
-- tree a graph, not strictly a tree).
local function max_line(n, seen)
   if type(n) ~= "table" or seen[n] then return 0 end
   seen[n] = true
   local m = 0
   if is_node(n) then
      if n.y and n.y > m then m = n.y end
      if n.yend and n.yend > m then m = n.yend end
   end
   for k, v in pairs(n) do
      if not SKIP_KEYS[k] and type(v) == "table" then
         local mv = max_line(v, seen)
         if mv > m then m = mv end
      end
   end
   return m
end

local function collect_spans(ast, tokens)
   local spans = {}
   local seen = {}
   local at = tokens_by_pos(tokens)
   local function go(n)
      if type(n) ~= "table" or seen[n] then return end
      seen[n] = true
      if is_node(n) then
         local kw = keyword_mark(n)
         -- One level for every line a marked node goes on to occupy after the keyword's
         -- line, wherever that node is — a statement, the expression of a declaration,
         -- a `return` — including any of its own multi-line spans (an argument list
         -- that does not close on its first line, a callback body passed into it, ...),
         -- which stack with this the same way a block body stacks with a bracket inside
         -- it.
         --
         -- `n.y` is the line of the token `n` is positioned at: the `(` of a call
         -- (`@funcall`, vendor/tl.lua:3335 positions it at its argument list's `y, x`),
         -- the `local` of an `async local` declaration, the variable of an awaited task
         -- (the generated `__X:await()` is placed at the variable's own position) — all
         -- of which the keyword always precedes in the source. `kw.y < n.y` is exactly
         -- "a line break lies between the keyword and that token", which is what this
         -- rule is for; `kw.y == n.y` is the one-line form (`await f(x)`, `async local
         -- t = f(x)`) and gets no span at all, because the paren span for `f(...)`'s
         -- own argument list and the block span for any callback body passed into it
         -- already count every line `f(...)` itself runs on, split or not.
         if kw and kw.y < n.y then
            spans[#spans + 1] = { kind = "kwcont", y1 = kw.y, y2 = max_line(n, {}) }
         end
         if n.yend then
            if n.kind == "statements" and n ~= ast then
               -- `y1`/`ox` is where the body opens and `by`/`x1` where its first
               -- statement is; the two differ exactly when something the parser does
               -- not tokenize (a comment) sits between them, when there is no
               -- statement at all, or when the first statement is one of the
               -- keyword-marked nodes above, whose own `y, x` is not a position the
               -- token stream has anything at (its `x` moved to the keyword's column,
               -- its `y` left at the line after it) — the keyword's own position is
               -- what the block actually starts at and what names its opener.
               local by, bx = n.y, n.x
               local first = n[1]
               local kw = is_node(first) and keyword_mark(first)
               if kw then by, bx = kw.y, kw.x end
               local oy, ox = opener_pos(at, tokens, by, bx)
               spans[#spans + 1] = { kind = "block", y1 = oy, ox = ox, by = by, x1 = bx, y2 = n.yend, x2 = n.xend or 0 }
            elseif n.kind == "literal_table" then
               spans[#spans + 1] = { kind = "brace", y1 = n.y, x1 = n.x, y2 = n.yend }
            elseif (n.kind == "argument_list" or n.kind == "expression_list") and n.tk == "(" then
               spans[#spans + 1] = { kind = "paren", y1 = n.y, x1 = n.x, y2 = n.yend }
            elseif n.kind == "newtype" then
               spans[#spans + 1] = { kind = "typeblock", y1 = n.y, y2 = n.yend }
            end
         end
      end
      for k, v in pairs(n) do
         if not SKIP_KEYS[k] and type(v) == "table" then go(v) end
      end
   end
   go(ast)
   return spans
end

---------------------------------------------------------------- tokens per line

local TYPE_OPENERS = { record = true, enum = true, interface = true }
local BIN_LAST = {
   [".."] = true, ["+"] = true, ["-"] = true, ["*"] = true, ["/"] = true, ["//"] = true, ["%"] = true,
   ["^"] = true, ["=="] = true, ["~="] = true, ["<"] = true, [">"] = true, ["<="] = true, [">="] = true,
   ["and"] = true, ["or"] = true, ["="] = true, ["|"] = true, ["&"] = true, ["<<"] = true, [">>"] = true,
}
local BIN_FIRST = {}
for k in pairs(BIN_LAST) do BIN_FIRST[k] = true end
BIN_FIRST["-"] = nil
BIN_FIRST["="] = nil

-- Does the line whose tokens these are end in an operator that leaves it unfinished?
--
-- `BIN_LAST` is keyed on token text, and the lexer hands out the same `>` for `a > b` and
-- for the close of `Box<T>` — so a line ending in a generic's closing angle (`record
-- Map<K, V>`, `function f<T>(): Box<T>`) would read as a comparison waiting for its right
-- side, and the line after it would get a continuation level it does not have. The two are
-- told apart on the line itself: a type-argument list opens with a `<` that directly
-- follows an identifier, and its close matches one of those. A trailing `>` (or `>>`, which
-- the lexer produces for the two closes of `Box<Box<T>>` and which is in the table as the
-- shift) is a continuation only when no such open is left for it to close. A comparison
-- that happens to sit on a line with a generic open (`if a < b and Box<T> >`) is read as a
-- close; Teal does not write that line, and the cost is one missing level rather than a
-- wrong one on every generic record.
local function ends_unfinished(toks)
   local last = toks[#toks]
   if not last or not BIN_LAST[last.tk] then
      return false
   end
   if last.tk ~= ">" and last.tk ~= ">>" then
      return true
   end
   local open = 0
   for i, t in ipairs(toks) do
      if t.tk == "<" and i > 1 and toks[i - 1].kind == "identifier" then
         open = open + 1
      elseif t.tk == ">" then
         open = open - 1
      elseif t.tk == ">>" then
         open = open - 2
      end
   end
   return open < 0
end

local function tokens_by_line(tokens)
   local by = {}
   for _, t in ipairs(tokens) do
      if t.kind ~= "$EOF$" and t.kind ~= "comment" and t.y then
         by[t.y] = by[t.y] or {}
         table.insert(by[t.y], t)
      end
   end
   return by
end

-- Extra depth inside type blocks from nested `record` / `enum` / `interface` ... `end`.
-- Returns map line -> nested depth at that line's start (after leading `end`s).
local function nested_type_depths(spans, by_line)
   local extra = {}
   for _, s in ipairs(spans) do
      if s.kind == "typeblock" then
         local nested = 0
         for L = s.y1 + 1, s.y2 - 1 do
            local toks = by_line[L] or {}
            local leading_end = 0
            for _, t in ipairs(toks) do
               if t.tk == "end" then leading_end = leading_end + 1 else break end
            end
            extra[L] = (extra[L] or 0) + math.max(nested - leading_end, 0)
            for _, t in ipairs(toks) do
               if TYPE_OPENERS[t.tk] then nested = nested + 1
               elseif t.tk == "end" then nested = nested - 1 end
            end
         end
      end
   end
   return extra
end

---------------------------------------------------------------- format

function F.format(src, filename, opts)
   opts = opts or {}
   local indent_w = opts.indent or 3
   local max_blank = opts.max_blank or 2
   filename = filename or "input.tl"

   local ast, errs = tl.parse(src, filename, "tl")
   if not ast or #errs > 0 then
      local e = errs[1]
      return nil, string.format("%s:%d:%d: %s", filename, e.y or 0, e.x or 0, e.msg or "syntax error")
   end
   local tokens = tl.lex(src, filename)
   local by_line = tokens_by_line(tokens)
   local spans = collect_spans(ast, tokens)
   local type_extra = nested_type_depths(spans, by_line)
   local prot = protected_lines(src)
   local lines = split_lines(src)

   -- Block terminators count as closers too: `end)` closes a callback argument.
   local CLOSERS = { [")"] = true, ["}"] = true, ["end"] = true, ["until"] = true }
   -- The span kinds `in_bracket` means a line to be "inside" of for the bracket count
   -- below. `kwcont` (`collect_spans`) shares `in_bracket`'s "on the end line but not the
   -- closer" shape by coincidence — its own line never starts with `)` / `}` / `end` /
   -- `until` either — so it has to be kept out of this count explicitly rather than by
   -- excluding only `block`, or a line in a marked node's `kwcont` span would be counted
   -- twice: once here and once by the keyword-continuation count below.
   local BRACKET_KINDS = { paren = true, brace = true, typeblock = true }
   -- A line is inside a bracket span when strictly between its start and end lines, or on
   -- the end line but not starting with the closer (`   c)` is still an argument line).
   local function in_bracket(s, L)
      if L <= s.y1 then return false end
      if L < s.y2 then return true end
      if L > s.y2 then return false end
      local toks = by_line[L]
      local first = toks and toks[1] and toks[1].tk
      return not (first and CLOSERS[first])
   end

   local function base_depth(L, firstX)
      -- Blocks: (y2, x2) is the end of the terminating token (`end` / `elseif` /
      -- `else` / `until`), so the terminator's line is never part of the body.
      local blocks = {}
      for _, s in ipairs(spans) do
         if s.kind == "block" then
            -- Past the line the body opens on, or on the line its first statement is on
            -- and not to the left of it (`if x then y = 1` puts `y = 1` inside and the
            -- `if` outside). The two tests are separate because the body's first statement
            -- is not always on the opening line, and a column from another line says
            -- nothing about this one.
            local after_start = (L > s.y1) or (L == s.by and firstX >= s.x1)
            if after_start and L < s.y2 then blocks[#blocks + 1] = s end
         end
      end
      -- Brackets: a `(` / `{` span counts once per start line, and a `(` span does not
      -- count inside a block that opened after it (callback bodies indent one level,
      -- not two: `f("x", function()` ... `end)`).
      --
      -- "After" is a position and not a line. A callback's body opens at the `)` of its
      -- own argument list, which is on the line the call opened on — so comparing lines
      -- alone would miss it, while a call *inside* a block body (`while c do g(` ... `)`)
      -- opens after that body did and has to keep counting.
      local bracket_lines = {}
      for _, s in ipairs(spans) do
         if BRACKET_KINDS[s.kind] and in_bracket(s, L) then
            local shadowed = false
            if s.kind == "paren" then
               for _, b in ipairs(blocks) do
                  if b.y1 > s.y1 or (b.y1 == s.y1 and b.ox > (s.x1 or 0)) then
                     shadowed = true
                     break
                  end
               end
            end
            if not shadowed then bracket_lines[s.y1] = true end
         end
      end
      -- Keyword continuation (`collect_spans`): the lines a marked node — a statement,
      -- the expression of a declaration or a `return` — goes on to occupy after the
      -- line that held only the keyword, one level each — the same reason a line ending
      -- in `+` gets a level for the next, except nothing on the line itself marks it, so
      -- `base_depth` is where it is counted rather than the sequential `prev_unfinished`
      -- check below. `L <= s.y2` rather than `<`: unlike a block's terminator, the
      -- node's own last line (the call's, or the closing `)` of one that wraps) is
      -- still part of what continues.
      local kw_lines = 0
      for _, s in ipairs(spans) do
         if s.kind == "kwcont" and L > s.y1 and L <= s.y2 then kw_lines = kw_lines + 1 end
      end
      local d = #blocks + kw_lines
      for _ in pairs(bracket_lines) do d = d + 1 end
      return d + (type_extra[L] or 0)
   end

   local out = {}
   local blank_run = 0
   local prev_unfinished = false -- did the previous code line end in an operator?
   for L, line in ipairs(lines) do
      if prot[L] then
         out[#out + 1] = line
         blank_run = 0
      else
         local content = line:gsub("^%s+", ""):gsub("%s+$", "")
         if content == "" then
            blank_run = blank_run + 1
            if blank_run <= max_blank and #out > 0 then out[#out + 1] = "" end
         else
            blank_run = 0
            local firstX = #line:match("^%s*") + 1
            local depth = base_depth(L, firstX)
            local toks = by_line[L]
            local first_tk = toks and toks[1] and toks[1].tk
            if prev_unfinished or (first_tk and BIN_FIRST[first_tk]) then
               depth = depth + 1
            end
            out[#out + 1] = string.rep(" ", depth * indent_w) .. content
            if toks and #toks > 0 then prev_unfinished = ends_unfinished(toks) end
         end
      end
   end
   -- drop trailing blank lines
   while #out > 0 and out[#out] == "" do out[#out] = nil end
   return table.concat(out, "\n") .. "\n"
end

return F
