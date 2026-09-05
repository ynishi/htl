-- htl.test: default assertion library for `htl test`.
-- Typed surface lives in test.d.tl; this is the runtime.
--
-- Contract with the runner (any library can implement it):
--   run(filter?: string) -> { passed: integer, failed: integer, failures: {string} }

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

---------------------------------------------------------------- run

local function format_error(err)
   if type(err) == "table" and err.htl_assert then return err.msg end
   return tostring(err)
end

function M.run(filter)
   local report = { passed = 0, failed = 0, failures = {} }
   for _, s in ipairs(suites) do
      for _, t in ipairs(s.tests) do
         local full = (s.name ~= "" and (s.name .. " > ") or "") .. t.name
         if not filter or full:find(filter, 1, true) then
            local ok, err = xpcall(t.fn, function(e)
               if type(e) == "table" and e.htl_assert then return e end
               return debug.traceback(tostring(e), 2)
            end)
            if ok then
               report.passed = report.passed + 1
            else
               report.failed = report.failed + 1
               report.failures[#report.failures + 1] = full .. ": " .. format_error(err)
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
