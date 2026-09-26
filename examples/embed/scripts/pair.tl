local host = require("host")
local http = require("http")

-- Two calls that each wait 300 ms, side by side: `async local` starts the first as a
-- task, `await` waits for the second, and the task's value is collected at the end.
local async function pair(a: string, b: string): string, string
   async local x = http.get(a)
   local y = await http.get(b)
   return await x, y
end

local t0 = host:uptime_ms()
print(await pair("/a", "/b"))
local took = host:uptime_ms() - t0
assert(took < 400, "pair took " .. tostring(took) .. " ms for two 300 ms calls")

-- A task not awaited when its scope ends is cancelled and waited for: the host's future
-- for "/slow" is dropped (it prints `dropped`) before the caller prints the result.
local async function leave(): string
   async local _slow = http.get("/slow")
   return await http.get("/fast")
end

print(await leave())
