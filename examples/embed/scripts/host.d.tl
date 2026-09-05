local record host
   record Point
      x: number
      y: number
   end
   now: function(self: host): number
   uptime_ms: function(self: host): integer
   scale: function(self: host, p: Point, k: number): Point
   greet: function(name: string): string
   sum: function(xs: {number}): number
   norm: function(self: host, p: Point): number
   parse_int: function(s: string): integer
end

return host
