local util = require("util")

local p = util.add({ x = 1, y = 2 }, { x = 10, y = 20 })
print(util.greet("htl"), p.x, p.y)

local args: {string} = { ... }
print("args:", #args, table.concat(args, " "))
