local task = require("htl.task")
local gate = require("gate")

-- The decision the host moved into Teal: a path under `/public/` may be served.
local function decide(call: gate.Call): gate.Decision
   if call.path:sub(1, #"/public/") == "/public/" then
      return { allow = true, reason = "" }
   end
   return { allow = false, reason = "not public" }
end

-- The host asks from a thread of its own and waits at most the kind's deadline. The loop
-- answers each question as it comes, and ends when the host drops its asker: the channel
-- closes, and the case sees `ok == false` once the queued questions are taken.
local calls = gate:calls()
local open = true
while open do
   open = await task.select({
      calls:on(async function(req: task.Request<gate.Call, gate.Decision>, ok: boolean): boolean
         if not ok then
            return false
         end
         if req.value.path == "/slow" then
            -- Longer than the deadline: the host has taken its default by now, and the
            -- late answer is discarded rather than delivered to the next question.
            await task.after(500):wait()
            assert(not req:reply(decide(req.value)), "a reply after the deadline was delivered")
         else
            req:reply(decide(req.value))
         end
         return true
      end),
   })
end
