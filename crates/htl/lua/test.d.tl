-- Type declarations for htl.test (runtime: test.lua).
local record test
   record Expect<T>
      to_equal: function(self, expected: T)
      to_not_equal: function(self, expected: T)
      to_be_truthy: function(self)
      to_be_falsy: function(self)
      to_be_nil: function(self)
      to_be_close: function(self, expected: number, eps?: number)
      to_error: function(self, pattern?: string)
   end

   record Report
      passed: integer
      failed: integer
      failures: {string}
   end

   describe: function(name: string, body: function())
   it: function(name: string, body: function())
   expect: function<T>(actual: T): Expect<T>
   run: function(filter?: string): Report
   count: function(): integer
end

return test
