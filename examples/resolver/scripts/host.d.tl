-- Declaration for the Rust-native `host` module (NativeResolver, registered before
-- TealResolver so this file only serves the checker).
local record host
   name: string
   double: function(n: number): number
end

return host
