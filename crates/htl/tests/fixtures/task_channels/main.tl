-- The host's main loop in Teal: events and requests from the host, taken in one select,
-- and a report sent back. `daemon.d.tl` types each channel from the Rust signature.
local task = require("htl.task")
local daemon = require("daemon")

local events = daemon:events()
local calls = daemon:calls()
local reports = daemon:reports()

local sum = 0
local answered = 0
local events_open, calls_open = true, true
while events_open or calls_open do
   -- A closed, empty channel stays ready, so a source leaves the list once it closes.
   local cases: {task.Case<boolean>} = {}
   if events_open then
      table.insert(cases, events:on(function(ev: daemon.Event, ok: boolean): boolean
         if ok then sum = sum + ev.n else events_open = false end
         return ok
      end))
   end
   if calls_open then
      table.insert(cases, calls:on(function(req: task.Request<string, integer>, ok: boolean): boolean
         if ok then
            req:reply(#req.value)
            answered = answered + 1
         else
            calls_open = false
         end
         return ok
      end))
   end
   await task.select(cases)
end

await reports:send({ text = "sum " .. sum .. ", answered " .. answered })
reports:close()
