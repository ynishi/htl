-- Deliberately failing tests (runtime assertion failures). Driven from
-- `tests/sample_project.rs`, which asserts that both failures are named and that the run
-- exits non-zero.
local t = require("htl.test")

t.describe("arithmetic", function()
   t.it("passes", function()
      t.expect(1 + 1):to_equal(2)
   end)

   t.it("fails on purpose", function()
      t.expect(1 + 1):to_equal(3)
   end)

   t.it("expected an error but none came", function()
      t.expect(function() end):to_error()
   end)
end)
