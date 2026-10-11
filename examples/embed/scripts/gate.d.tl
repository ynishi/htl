local type task = require("htl.task")

local record gate
   record Call
      path: string
   end
   record Decision
      allow: boolean
      reason: string
   end
   calls: function(self: gate): task.RecvChannel<task.Request<Call, Decision>>
end

return gate
