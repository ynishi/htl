local util = require("util")
local host = require("host")

local p = util.add({ x = 1, y = 2 }, { x = 3, y = 4 })
print(util.greet("embed"), p.x, p.y)

local t: number = host:now()
print("host:now():", t)
print("host:uptime_ms():", host:uptime_ms())

local q: host.Point = host:scale({ x = 1.5, y = -2 }, 4)
print("host:scale():", q.x, q.y)
print("host:norm():", host:norm({ x = 3, y = 4 }))

print(host.greet("teal"))
print("host.sum:", host.sum({ 1, 2, 3.5 }))
print("host.parse_int('42'):", host.parse_int(" 42 "))

local ok, err = pcall(host.parse_int, "nope")
print("host.parse_int('nope') ->", ok, err)

-- errors = "return": `value, err` instead of raising
local store = require("store")
local wrote, werr = store:write("htl-embed-demo.txt", "hello")
print("store:write ->", wrote, werr)
local text, rerr = store:read("htl-embed-demo.txt")
print("store:read ->", text, rerr)
local bad, berr = store:write("no/slash", "x")
print("store:write bad ->", bad, berr)

local args: {string} = { ... }
print("args:", #args, table.concat(args, " "))
