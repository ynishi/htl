-- htl lint samples for shadow-local / no-global / no-any (no-any is opt-in: --lint +no-any).

global VERSION = "1.0"

local count = 0

local function bump(n: number): number
   local count = n + 1 -- shadows the module-level `count`
   return count
end

local function each(xs: {number})
   for _, count in ipairs(xs) do -- loop var shadows too
      print(count)
   end
end

local function loose(v: any): any
   return v
end

local function ok(v: number): number
   local inner = v -- htl: allow(shadow-local)
   return inner
end

print(bump(count), VERSION, loose(1), ok(2))
each({ 1, 2 })
