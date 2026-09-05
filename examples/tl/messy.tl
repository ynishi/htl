-- htl fmt sample: indentation is wrong on purpose; `htl fmt` recomputes it.
local record Shape
      kind: string
   record Size
   w: number
        h: number
      end
  area: function(self: Shape): number
end



local function describe(s: Shape, sizes: {Shape.Size}): string
if s.kind == "box" then
local total: number = 0
      for _, sz in ipairs(sizes) do
   total = total + sz.w * sz.h
end
        return "box " .. total
   elseif s.kind == "dot" then
 return "dot"
else
    return "shape:" ..
    s.kind
end
end

local long = [[
   this long string
      keeps its own indentation
]]

local t = {
       a = 1,
  b = {
 c = 2,
   },
}
print(describe({ kind = "box" }, { { w = 2, h = 3 } }), long, t.a)
