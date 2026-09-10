local t = require("htl.test")
local util = require("util")

t.describe("util.add", function()
   t.it("adds componentwise", function()
      t.expect(util.add({ x = 1, y = 2 }, { x = 10, y = 20 })):to_equal({ x = 11, y = 22 })
   end)

   t.it("is commutative", function()
      local a: util.Point = { x = 3, y = -1 }
      local b: util.Point = { x = 0.5, y = 4 }
      t.expect(util.add(a, b)):to_equal(util.add(b, a))
   end)
end)

t.describe("util.greet", function()
   t.it("prefixes hello", function()
      t.expect(util.greet("teal")):to_equal("hello, teal")
   end)

   t.it("raises on nil at runtime", function()
      t.expect(function()
         local f = util.greet as function(any): string
         f(nil)
      end):to_error("concatenate")
   end)
end)
