--- The engine: balls in a box. Pure data and rules, so `htl test` runs it with no window;
--- only `render` talks to `mq` and `fx`, and nothing here calls it.
local mq = require("mq")
local fx = require("fx")

local record {{mod}}
   record Ball
      x: number
      y: number
      vx: number
      vy: number
      r: number
   end
   record Scene
      balls: {Ball}
   end
end

--- `n` balls at the centre of a `w` by `h` box, fanned out.
function {{mod}}.new_scene(n: integer, w: number, h: number): {{mod}}.Scene
   local balls: {{{mod}}.Ball} = {}
   for i = 1, n do
      local a = (i / n) * 2 * math.pi
      balls[i] = { x = w / 2, y = h / 2, vx = 160 * math.cos(a), vy = 160 * math.sin(a), r = 14 }
   end
   return { balls = balls }
end

--- Advance every ball by `dt` seconds, bouncing off the walls of a `w` by `h` box.
function {{mod}}.step(s: {{mod}}.Scene, dt: number, w: number, h: number)
   for _, b in ipairs(s.balls) do
      b.x = b.x + b.vx * dt
      b.y = b.y + b.vy * dt
      if b.x < b.r then b.x = b.r; b.vx = -b.vx end
      if b.x > w - b.r then b.x = w - b.r; b.vx = -b.vx end
      if b.y < b.r then b.y = b.r; b.vy = -b.vy end
      if b.y > h - b.r then b.y = h - b.r; b.vy = -b.vy end
   end
end

--- Draw the scene: the glow is `fx`'s, the rest is `mq`'s.
function {{mod}}.render(s: {{mod}}.Scene)
   mq:clear_background({ r = 0.08, g = 0.08, b = 0.12, a = 1 })
   for _, b in ipairs(s.balls) do
      fx:draw_glow(b.x, b.y, b.r, { r = 1, g = 0.6, b = 0.2, a = 1 })
   end
   mq:draw_text("fps " .. mq:fps() .. "  space: spawn  esc: quit", 16, 32, 24, { r = 1, g = 1, b = 1, a = 1 })
end

return {{mod}}
