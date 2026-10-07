local type task = require("htl.task")

local record daemon
   record Event
      n: integer
   end
   record Report
      text: string
   end
   events: function(self: daemon): task.RecvChannel<Event>
   calls: function(self: daemon): task.RecvChannel<task.Request<string, integer>>
   reports: function(self: daemon): task.SendChannel<Report>
   is_reports: function(self: daemon, ch: task.SendChannel<Report>): boolean
end

return daemon
