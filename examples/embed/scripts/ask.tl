local task = require("htl.task")
local gate = require("gate")

-- The decision the host moved into Teal: a path under `/public/` may be served.
-- `requestPath` is the name the wire has: `Call` is a serde type with
-- `#[serde(rename = "requestPath")] path`, so `gate.Call` declares the key serde writes,
-- and the Rust name, `call.path`, is a check error here.
local function decide(call: gate.Call): gate.Decision
   if call.requestPath:sub(1, #"/public/") == "/public/" then
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
         if req.value.requestPath:sub(1, 1) ~= "/" then
            -- Not a question the gate answers: refused with a reason, which reaches the
            -- host as `AskError::Rejected` — told apart from a deny the gate decided.
            assert(req:reject("path must start with /"), "a reject in time was not delivered")
         elseif req.value.requestPath == "/slow" then
            -- Longer than the deadline: the host has taken its default by now, and the
            -- late answer is discarded rather than delivered to the next question.
            await task.after(500):wait()
            assert(not req:reply(decide(req.value)), "a reply after the deadline was delivered")
         else
            -- The ordinary path: `answer` replies with what `decide` returns, and would
            -- reject with the error if `decide` raised, so a bug reaches the host as
            -- `AskError::Rejected` rather than as a timeout.
            await task.answer(req, decide)
         end
         return true
      end),
   })
end
