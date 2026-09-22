local t = require("htl.test")
local {{mod}} = require("{{mod}}")

t.describe("{{mod}}", function()
   t.it("a ball moves and bounces", function()
      -- One ball starts at the centre going right at 160/s, so half a second is +80.
      local s = {{mod}}.new_scene(1, 200, 200)
      {{mod}}.step(s, 0.5, 200, 200)
      local b = s.balls[1]
      t.expect(b.x):to_be_close(180)
      -- Another +80 would be past the wall at 200 - r, so it is put on the wall instead
      -- and turned round; the step after that carries it back into the box.
      {{mod}}.step(s, 0.5, 200, 200)
      t.expect(b.x):to_be_close(186)
      t.expect(b.vx):to_be_less_than(0)
      {{mod}}.step(s, 0.5, 200, 200)
      t.expect(b.x):to_be_close(106)
   end)

   t.it("the scene has the balls it was asked for", function()
      t.expect(#{{mod}}.new_scene(3, 800, 600).balls):to_equal(3)
   end)
end)
