-- htl.test: default assertion library for `htl test`.
-- Typed surface lives in test.d.tl; this is the runtime.
--
-- Contract with the runner (any library can implement it):
--   run(filter?: string, opts?: { fail_fast: boolean })
--     -> { passed: integer, failed: integer, failures: {string},
--          tests: { { name: string, ok: boolean, ms: number } },      (optional)
--          snapshots_written: {string}, snapshots_updated: {string} }  (optional)
--   configure({ snapshot_dir, update, mkdir })   (optional; called before the file runs)
--
-- Kept deliberately small (Go's `testing` / Rust's `assert_eq!` rather than Jest): the
-- runner is where htl invests; the assertion surface is a handful of matchers that
-- read well in a type-checked file.

local M = {}

local suites = {}      -- { { name = string, tests = { {name=, fn=} } } }
local current = nil
local top = { name = "", tests = {} }
suites[1] = top

function M.describe(name, body)
   local prev = current
   current = { name = name, tests = {} }
   suites[#suites + 1] = current
   body()
   current = prev
end

function M.it(name, fn)
   local s = current or top
   s.tests[#s.tests + 1] = { name = name, fn = fn }
end

---------------------------------------------------------------- expect

local function deep_equal(a, b)
   if a == b then return true end
   if type(a) ~= "table" or type(b) ~= "table" then return false end
   for k, v in pairs(a) do
      if not deep_equal(v, b[k]) then return false end
   end
   for k in pairs(b) do
      if a[k] == nil then return false end
   end
   return true
end

local function show(v)
   if type(v) == "string" then return string.format("%q", v) end
   if type(v) ~= "table" then return tostring(v) end
   local parts = {}
   local n = 0
   for k, x in pairs(v) do
      n = n + 1
      if n > 8 then parts[#parts + 1] = "..." break end
      parts[#parts + 1] = tostring(k) .. "=" .. show(x)
   end
   return "{" .. table.concat(parts, ", ") .. "}"
end

local function fail(msg)
   error({ htl_assert = true, msg = msg }, 3)
end

local Expect = {}
Expect.__index = Expect

function Expect:to_equal(expected)
   if not deep_equal(self.actual, expected) then
      fail("expected " .. show(expected) .. ", got " .. show(self.actual))
   end
end

function Expect:to_not_equal(expected)
   if deep_equal(self.actual, expected) then
      fail("expected value to differ from " .. show(expected))
   end
end

function Expect:to_be_truthy()
   if not self.actual then fail("expected truthy, got " .. show(self.actual)) end
end

function Expect:to_be_falsy()
   if self.actual then fail("expected falsy, got " .. show(self.actual)) end
end

function Expect:to_be_nil()
   if self.actual ~= nil then fail("expected nil, got " .. show(self.actual)) end
end

function Expect:to_not_be_nil()
   if self.actual == nil then fail("expected a value, got nil") end
end

function Expect:to_be_close(expected, eps)
   eps = eps or 1e-9
   local a = self.actual
   if type(a) ~= "number" or math.abs(a - expected) > eps then
      fail("expected " .. tostring(expected) .. " (±" .. tostring(eps) .. "), got " .. show(a))
   end
end

function Expect:to_be_greater_than(n)
   local a = self.actual
   if type(a) ~= "number" or not (a > n) then
      fail("expected a number greater than " .. tostring(n) .. ", got " .. show(a))
   end
end

function Expect:to_be_less_than(n)
   local a = self.actual
   if type(a) ~= "number" or not (a < n) then
      fail("expected a number less than " .. tostring(n) .. ", got " .. show(a))
   end
end

function Expect:to_be_at_least(n)
   local a = self.actual
   if type(a) ~= "number" or not (a >= n) then
      fail("expected a number of at least " .. tostring(n) .. ", got " .. show(a))
   end
end

function Expect:to_be_at_most(n)
   local a = self.actual
   if type(a) ~= "number" or not (a <= n) then
      fail("expected a number of at most " .. tostring(n) .. ", got " .. show(a))
   end
end

-- Substring of a string (plain, not a pattern), or an element of an array.
function Expect:to_contain(x)
   local a = self.actual
   if type(a) == "string" then
      if type(x) ~= "string" or not a:find(x, 1, true) then
         fail("expected " .. show(a) .. " to contain " .. show(x))
      end
      return
   end
   if type(a) == "table" then
      for _, v in ipairs(a) do
         if deep_equal(v, x) then return end
      end
      fail("expected array " .. show(a) .. " to contain " .. show(x))
   end
   fail("to_contain needs a string or an array, got " .. show(a))
end

-- Absent from a string (plain), or from an array; the array message names the index.
function Expect:to_not_contain(x)
   local a = self.actual
   if type(a) == "string" then
      if type(x) == "string" and a:find(x, 1, true) then
         fail("expected " .. show(a) .. " not to contain " .. show(x))
      end
      return
   end
   if type(a) == "table" then
      for i, v in ipairs(a) do
         if deep_equal(v, x) then
            fail("expected array " .. show(a) .. " not to contain " .. show(x) .. " (found at index " .. tostring(i) .. ")")
         end
      end
      return
   end
   fail("to_not_contain needs a string or an array, got " .. show(a))
end

-- Lua pattern match on a string.
function Expect:to_match(pattern)
   local a = self.actual
   if type(a) ~= "string" or not a:find(pattern) then
      fail("expected " .. show(a) .. " to match /" .. tostring(pattern) .. "/")
   end
end

function Expect:to_not_match(pattern)
   local a = self.actual
   if type(a) ~= "string" then
      fail("to_not_match needs a string, got " .. show(a))
   end
   local s, e = a:find(pattern)
   if s then
      fail("expected " .. show(a) .. " not to match /" .. tostring(pattern) .. "/ (matched " .. show(a:sub(s, e)) .. " at " .. tostring(s) .. ")")
   end
end

function Expect:to_have_length(n)
   local a = self.actual
   if (type(a) ~= "string" and type(a) ~= "table") or #a ~= n then
      fail("expected length " .. tostring(n) .. ", got " .. show(a))
   end
end

function Expect:to_error(pattern)
   local fn = self.actual
   if type(fn) ~= "function" then fail("to_error needs a function, got " .. show(fn)) end
   local ok, err = pcall(fn)
   if ok then fail("expected an error, but the function returned") end
   if pattern and not tostring(err):find(pattern) then
      fail("error did not match /" .. pattern .. "/: " .. tostring(err))
   end
end

---------------------------------------------------------------- snapshots

-- Set by the runner before the file runs (`M.configure`): where this file's
-- snapshots live, whether to rewrite differing ones, and how to create the dir.
local snap = { dir = nil, update = false, mkdir = nil, used = {}, written = {}, updated = {} }

function M.configure(cfg)
   snap.dir = cfg.snapshot_dir
   snap.update = cfg.update and true or false
   snap.mkdir = cfg.mkdir
end

--- The run's random stream, as a function with `math.random`'s own shape:
--- `rng()`, `rng(m)`, `rng(m, n)`.
---
--- The runner seeds the state before the file runs, so this and `math.random` are one
--- stream and both repeat under `htl test --seed <n>`. Drawing through this rather than
--- reaching for `math.random` says in the test that the values are meant to be
--- reproducible, and leaves the runner somewhere to change how they are produced.
---
--- A test that calls `math.randomseed` itself takes the stream over from that point; the
--- runner does not seed again.
function M.rng()
   return math.random
end

-- Deterministic text for a value: sorted keys, one entry per line, so a snapshot
-- diff is readable and stable across runs.
local function serialize(v, indent)
   if type(v) == "string" then return string.format("%q", v) end
   if type(v) ~= "table" then return tostring(v) end
   local keys = {}
   for k in pairs(v) do keys[#keys + 1] = k end
   table.sort(keys, function(a, b)
      local ta, tb = type(a), type(b)
      if ta ~= tb then return ta < tb end
      return a < b
   end)
   if #keys == 0 then return "{}" end
   local out = { "{" }
   for _, k in ipairs(keys) do
      local key = type(k) == "string" and k:match("^[%a_][%w_]*$") and k or ("[" .. serialize(k, "") .. "]")
      out[#out + 1] = indent .. "  " .. key .. " = " .. serialize(v[k], indent .. "  ") .. ","
   end
   out[#out + 1] = indent .. "}"
   return table.concat(out, "\n")
end

-- A string is stored as is; an array of strings as its lines (a rendered screen);
-- anything else serialized. Always newline-terminated.
local function snapshot_text(v)
   if type(v) == "string" then
      return v:sub(-1) == "\n" and v or (v .. "\n")
   end
   if type(v) == "table" then
      local n = 0
      local lines = true
      for k, x in pairs(v) do
         n = n + 1
         if type(k) ~= "number" or type(x) ~= "string" then lines = false end
      end
      if lines and n == #v then
         return n == 0 and "" or (table.concat(v, "\n") .. "\n")
      end
   end
   return serialize(v, "") .. "\n"
end

local function split_lines(s)
   local out = {}
   for line in (s .. "\n"):gmatch("([^\n]*)\n") do out[#out + 1] = line end
   if out[#out] == "" then out[#out] = nil end
   return out
end

-- Line diff (LCS), printed as `-` / `+` lines with two lines of context.
local function diff(expected, actual)
   local a, b = split_lines(expected), split_lines(actual)
   local n, m = #a, #b
   local L = {}
   for i = n + 1, 1, -1 do
      L[i] = {}
      for j = m + 1, 1, -1 do
         if i > n or j > m then
            L[i][j] = 0
         elseif a[i] == b[j] then
            L[i][j] = L[i + 1][j + 1] + 1
         else
            L[i][j] = math.max(L[i + 1][j], L[i][j + 1])
         end
      end
   end
   local ops = {}
   local i, j = 1, 1
   while i <= n or j <= m do
      if i <= n and j <= m and a[i] == b[j] then
         ops[#ops + 1] = { " ", a[i] }; i, j = i + 1, j + 1
      elseif i <= n and (j > m or L[i + 1][j] >= L[i][j + 1]) then
         ops[#ops + 1] = { "-", a[i] }; i = i + 1 -- removals before additions, as diff prints them
      else
         ops[#ops + 1] = { "+", b[j] }; j = j + 1
      end
   end
   local keep = {}
   for k, op in ipairs(ops) do
      if op[1] ~= " " then
         for c = math.max(1, k - 2), math.min(#ops, k + 2) do keep[c] = true end
      end
   end
   local out, last = {}, 0
   for k, op in ipairs(ops) do
      if keep[k] then
         if k > last + 1 then out[#out + 1] = "@@" end
         out[#out + 1] = op[1] .. op[2]
         last = k
      end
   end
   return table.concat(out, "\n")
end

local function write_snapshot(path, text)
   local fd = io.open(path, "wb")
   if not fd and snap.mkdir then
      snap.mkdir(snap.dir)
      fd = io.open(path, "wb")
   end
   if not fd then fail("cannot write snapshot " .. path) end
   fd:write(text)
   fd:close()
end

function Expect:to_match_snapshot(name)
   if type(name) ~= "string" or name == "" then fail("to_match_snapshot needs a name") end
   if not snap.dir then
      fail("to_match_snapshot needs the runner: run this file with `htl test`")
   end
   local key = name:gsub("[^%w%-%._]+", "_")
   if snap.used[key] then fail("snapshot name '" .. name .. "' is used twice in this file") end
   snap.used[key] = true
   local path = snap.dir .. "/" .. key .. ".snap"
   local actual = snapshot_text(self.actual)
   local fd = io.open(path, "rb")
   if not fd then
      write_snapshot(path, actual)
      snap.written[#snap.written + 1] = path
      return
   end
   local expected = fd:read("a")
   fd:close()
   if expected == actual then return end
   if snap.update then
      write_snapshot(path, actual)
      snap.updated[#snap.updated + 1] = path
      return
   end
   fail("snapshot '" .. name .. "' differs from " .. path .. " (-expected +actual; `htl test --update` accepts the new value):\n"
      .. diff(expected, actual))
end

function M.expect(actual)
   return setmetatable({ actual = actual }, Expect)
end

-- Two-value results (`ok, why = f()`): `t.expect_all(f()):to_equal(false, "no door")`.
-- The type declaration fixes the arity at two, which is what a multi-value call in
-- argument position needs to be type-checked at all.
local Expect2 = {}
Expect2.__index = Expect2

function Expect2:to_equal(a, b)
   if not deep_equal(self.a, a) or not deep_equal(self.b, b) then
      fail("expected (" .. show(a) .. ", " .. show(b) .. "), got (" .. show(self.a) .. ", " .. show(self.b) .. ")")
   end
end

function M.expect_all(a, b)
   return setmetatable({ a = a, b = b }, Expect2)
end

---------------------------------------------------------------- run

local function format_error(err)
   if type(err) == "table" and err.htl_assert then return err.msg end
   return tostring(err)
end

function M.run(filter, opts)
   opts = opts or {}
   local report = { passed = 0, failed = 0, failures = {}, tests = {},
      snapshots_written = snap.written, snapshots_updated = snap.updated }
   for _, s in ipairs(suites) do
      for _, t in ipairs(s.tests) do
         local full = (s.name ~= "" and (s.name .. " > ") or "") .. t.name
         if not filter or full:find(filter, 1, true) then
            local t0 = os.clock()
            local ok, err = xpcall(t.fn, function(e)
               if type(e) == "table" and e.htl_assert then return e end
               return debug.traceback(tostring(e), 2)
            end)
            local ms = (os.clock() - t0) * 1000
            report.tests[#report.tests + 1] = { name = full, ok = ok, ms = ms }
            if ok then
               report.passed = report.passed + 1
            else
               report.failed = report.failed + 1
               report.failures[#report.failures + 1] = full .. ": " .. format_error(err)
               if opts.fail_fast then return report end
            end
         end
      end
   end
   return report
end

-- Number of registered tests (lets the runner tell "no tests" from "all filtered out").
function M.count()
   local n = 0
   for _, s in ipairs(suites) do n = n + #s.tests end
   return n
end

return M
