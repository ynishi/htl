-- Types only, nothing to run: imported with `local type shape = require("shape")`, which
-- the generator erases, so nothing asks for this name at run time.
local record shape
   record Point
      x: number
      y: number
   end
end

return shape
