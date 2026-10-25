local types = require("shop.types")

local record app
   split: function(s: string): string, integer
   greet: function(name: string, title: string | nil): types.Output
   limit: integer
end

function app.split(s: string): string, integer
   return s, #s
end

function app.greet(name: string, title: string | nil): types.Output
   return { ok = true, text = (title or "") .. name }
end

return app
