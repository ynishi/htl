local record fs
   read: function(path: string): string ---@async
   write: function(path: string, text: string) ---@async
   read_binary: function(path: string): string ---@async
   write_binary: function(path: string, bytes: string) ---@async
   exists: function(path: string): boolean ---@async
   is_file: function(path: string): boolean ---@async
   is_dir: function(path: string): boolean ---@async
   mkdir: function(path: string) ---@async
   remove: function(path: string) ---@async
   walk: function(path: string): {string} ---@async
end

return fs
