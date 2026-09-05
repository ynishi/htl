local util = require("util")

-- type error on purpose: greet returns string, not number
local n: number = util.greet("x")
print(n + 1)
