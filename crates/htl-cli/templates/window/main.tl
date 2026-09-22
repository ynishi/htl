local mq = require("mq")
local {{mod}} = require("{{mod}}")

local scene = {{mod}}.new_scene(8, 800, 600)

return {
   update = function(dt: number): boolean
      if mq:is_key_pressed("Space") then
         local more = {{mod}}.new_scene(4, mq:screen_width(), mq:screen_height())
         for _, b in ipairs(more.balls) do table.insert(scene.balls, b) end
      end
      {{mod}}.step(scene, dt, mq:screen_width(), mq:screen_height())
      return not mq:is_key_pressed("Escape")
   end,
   draw = function()
      {{mod}}.render(scene)
   end,
}
