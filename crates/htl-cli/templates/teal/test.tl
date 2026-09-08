local t = require("htl.test")
local {{mod}} = require("{{mod}}")

t.describe("{{mod}}.greet", function()
   t.it("greets by name", function()
      t.expect({{mod}}.greet("teal")):to_equal({ who = "teal", text = "hello, teal" })
   end)
end)
