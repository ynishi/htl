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

-- enum in, newtype out: a string each way, checked against host.Mode
local label: host.Label = host:pace("Careful")
print("host:pace('Careful'):", label)

-- data enum in: a table tagged with `kind`
print("host:area(circle 2):", host:area({ kind = "Circle", value = 2 } as host.Shape_Circle))
print("host:area(rect 3x4):", host:area({ kind = "Rect", w = 3, h = 4 } as host.Shape_Rect))

-- data enum out: narrowed with `is`, and union-exhaustive wants every variant seen
local function describe(s: host.Shape): string
   if s is host.Shape_Dot then
      return "a dot"
   elseif s is host.Shape_Circle then
      return "a circle of radius " .. tostring(s.value)
   elseif s is host.Shape_Rect then
      return "a " .. tostring(s.w) .. "x" .. tostring(s.h) .. " rect"
   end
end
print("host:bounding(2, 2):", describe(host:bounding(2, 2)))
print("host:bounding(3, 4):", describe(host:bounding(3, 4)))

-- a value the enum does not have is refused by name at run time
local okm, errm = pcall(host.pace, host, "fst" as host.Mode)
print("host:pace('fst') ->", okm, tostring(errm):match("^[^\n]*"))

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
