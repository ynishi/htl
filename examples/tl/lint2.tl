-- htl lint samples for shadow-local / no-global / no-any (no-any is opt-in: --lint +no-any).
-- `shadow-local` is the module case only: shadowing a plain local is reported by the Teal
-- compiler as `tl:redeclaration`, which names the line and column of what it shadows.

local util = require("util")

global VERSION = "1.0"

local count = 0

local function bump(n: number): number
   local count = n + 1 -- tl:redeclaration: shadows the module-level `count`
   return count
end

local function each(xs: {number})
   for _, count in ipairs(xs) do -- a loop variable redeclares too
      print(count)
   end
end

local function loose(v: any): any
   return v
end

-- shadow-local: for the rest of this function `util` is the string, and the module
-- required above cannot be reached.
local function greet(util: string): string
   return "hello, " .. util
end

print(bump(count), VERSION, loose(1), greet("world"), util.greet("again"))
each({ 1, 2 })
