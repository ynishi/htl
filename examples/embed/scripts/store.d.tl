local record store
   write: function(self: store, name: string, text: string): boolean, string
   read: function(self: store, name: string): string, string
end

return store
