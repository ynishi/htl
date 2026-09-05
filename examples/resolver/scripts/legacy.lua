-- Plain Lua module, served by FsResolver (TealResolver returns None for it).
local M = {}

function M.hello(name)
   return "hello, " .. name .. " (from legacy.lua)"
end

return M
