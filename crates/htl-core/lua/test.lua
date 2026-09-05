-- htl.test: default assertion library for `htl test`.
-- Typed surface lives in test.d.tl; this is the runtime.
--
-- Contract with the runner (any library can implement it):
--   run(filter?: string, opts?: { fail_fast: boolean })
--     -> { passed: integer, failed: integer, failures: {string},
--          tests: { { name: string, ok: boolean, ms: number } } }   (tests is optional)
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

-- Lua pattern match on a string.
function Expect:to_match(pattern)
   local a = self.actual
   if type(a) ~= "string" or not a:find(pattern) then
      fail("expected " .. show(a) .. " to match /" .. tostring(pattern) .. "/")
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
   local report = { passed = 0, failed = 0, failures = {}, tests = {} }
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
