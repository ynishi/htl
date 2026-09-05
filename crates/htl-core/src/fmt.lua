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

local function collect_spans(ast)
   local spans = {}
   local seen = {}
   local function go(n)
      if type(n) ~= "table" or seen[n] then return end
      seen[n] = true
      if is_node(n) and n.yend then
         if n.kind == "statements" and n ~= ast then
            spans[#spans + 1] = { kind = "block", y1 = n.y, x1 = n.x, y2 = n.yend, x2 = n.xend or 0 }
         elseif n.kind == "literal_table" then
            spans[#spans + 1] = { kind = "brace", y1 = n.y, y2 = n.yend }
         elseif (n.kind == "argument_list" or n.kind == "expression_list") and n.tk == "(" then
            spans[#spans + 1] = { kind = "paren", y1 = n.y, y2 = n.yend }
         elseif n.kind == "newtype" then
            spans[#spans + 1] = { kind = "typeblock", y1 = n.y, y2 = n.yend }
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
   local spans = collect_spans(ast)
   local type_extra = nested_type_depths(spans, by_line)
   local prot = protected_lines(src)
   local lines = split_lines(src)

   -- Block terminators count as closers too: `end)` closes a callback argument.
   local CLOSERS = { [")"] = true, ["}"] = true, ["end"] = true, ["until"] = true }
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
            local after_start = (L > s.y1) or (L == s.y1 and firstX >= s.x1)
            if after_start and L < s.y2 then blocks[#blocks + 1] = s end
         end
      end
      -- Brackets: a `(` / `{` span counts once per start line, and a `(` span does not
      -- count inside a block that opened after it (callback bodies indent one level,
      -- not two: `f("x", function()` ... `end)`).
      local bracket_lines = {}
      for _, s in ipairs(spans) do
         if s.kind ~= "block" and in_bracket(s, L) then
            local shadowed = false
            if s.kind == "paren" then
               for _, b in ipairs(blocks) do
                  if b.y1 > s.y1 then shadowed = true break end
               end
            end
            if not shadowed then bracket_lines[s.y1] = true end
         end
      end
      local d = #blocks
      for _ in pairs(bracket_lines) do d = d + 1 end
      return d + (type_extra[L] or 0)
   end

   local out = {}
   local blank_run = 0
   local prev_last_tk = nil -- last token of the previous code line
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
            if (prev_last_tk and BIN_LAST[prev_last_tk]) or (first_tk and BIN_FIRST[first_tk]) then
               depth = depth + 1
            end
            out[#out + 1] = string.rep(" ", depth * indent_w) .. content
            if toks and #toks > 0 then prev_last_tk = toks[#toks].tk end
         end
      end
   end
   -- drop trailing blank lines
   while #out > 0 and out[#out] == "" do out[#out] = nil end
   return table.concat(out, "\n") .. "\n"
end

return F
