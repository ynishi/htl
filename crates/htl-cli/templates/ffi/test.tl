local t = require("htl.test")
local {{mod}} = require("{{mod}}")

t.describe("{{mod}}.greet", function()
   t.it("greets by name", function()
      t.expect({{mod}}.greet("teal")):to_equal({ who = "teal", text = "hello, teal" })
   end)

   -- The failure the C and Python examples show: `htl test` covers it here, where the
   -- message is readable, and the examples show what it looks like across the boundary.
   t.it("refuses an empty name", function()
      t.expect(function() {{mod}}.greet("") end):to_error("`who` is empty")
   end)
end)
