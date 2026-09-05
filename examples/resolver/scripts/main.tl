local host = require("host")
local util = require("util")
local legacy = require("legacy")
local shape = require("shape")

local p: shape.Point = util.origin()
p = util.shift(p, 2, 3)
print(host.name, "shift:", p.x, p.y)
print("host.double(21):", host.double(21))
print("legacy.lua:", legacy.hello("teal"))

local ok, err = pcall(require, "broken")
print("require('broken') ->", ok)
print(err)
