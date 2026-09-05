local shape = require("shape")

local record util
end

function util.origin(): shape.Point
   return { x = 0, y = 0 }
end

function util.shift(p: shape.Point, dx: number, dy: number): shape.Point
   return { x = p.x + dx, y = p.y + dy }
end

return util
