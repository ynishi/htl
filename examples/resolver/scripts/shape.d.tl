-- Type-only module: TealResolver hands `require("shape")` an empty table at runtime.
local record shape
   record Point
      x: number
      y: number
   end
end

return shape
