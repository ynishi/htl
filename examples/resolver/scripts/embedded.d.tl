-- Declaration for a module the Rust side holds as a string (MemoryResolver, last in the
-- chain). Nothing on the filesystem implements it: TealResolver reads this for the
-- checker, steps aside at run time, and the chain carries on to the memory resolver.
local record embedded
   hello: function(name: string): string
end

return embedded
