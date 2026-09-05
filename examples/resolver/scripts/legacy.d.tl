-- Declaration for legacy.lua. At runtime TealResolver sees the sibling .lua and steps
-- aside, so FsResolver serves the real module.
local record legacy
   hello: function(name: string): string
end

return legacy
