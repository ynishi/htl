local record host
   record Point
      x: number
      y: number
   end
   enum Mode
      "Fast"
      "Careful"
   end
   type Label = string
   record Shape_Dot
      where self.kind == "Dot"
      kind: string
   end
   record Shape_Circle
      where self.kind == "Circle"
      kind: string
      value: number
   end
   record Shape_Rect
      where self.kind == "Rect"
      kind: string
      w: number
      h: number
   end
   type Shape = Shape_Dot | Shape_Circle | Shape_Rect
   now: function(self: host): number
   uptime_ms: function(self: host): integer
   scale: function(self: host, p: Point, k: number): Point
   greet: function(name: string): string
   sum: function(xs: {number}): number
   norm: function(self: host, p: Point): number
   parse_int: function(s: string): integer
   pace: function(self: host, m: Mode): Label
   area: function(self: host, s: Shape): number
   bounding: function(self: host, w: number, h: number): Shape
end

return host
