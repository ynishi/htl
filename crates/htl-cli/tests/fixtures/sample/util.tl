local record util
   record Point
      x: number
      y: number
   end
end

function util.add(a: util.Point, b: util.Point): util.Point
   return { x = a.x + b.x, y = a.y + b.y }
end

function util.greet(name: string): string
   return "hello, " .. name
end

return util
