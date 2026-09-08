local dep = require("dep")

local function hello(): string
   return dep:greet("world")
end

return hello
