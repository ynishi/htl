local {{mod}} = require("{{mod}}")

local g = {{mod}}.greet(arg and arg[1] or "teal")
print(g.text)
