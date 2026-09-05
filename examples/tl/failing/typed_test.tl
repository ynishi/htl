-- Deliberate *type* error in a test: Expect<integer>:to_equal wants an integer.
-- `htl test` refuses to run this file (the assertion is wrong before it ever executes).
local t = require("htl.test")

t.it("compares apples to strings", function()
   t.expect(1 + 1):to_equal("2")
end)
