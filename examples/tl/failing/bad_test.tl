-- Deliberately failing tests (runtime assertion failures), for `htl test examples/tl/failing`.
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
