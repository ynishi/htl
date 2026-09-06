-- Type declarations for htl.test (runtime: test.lua).
local record test
   record Expect<T>
      to_equal: function(self, expected: T)
      to_not_equal: function(self, expected: T)
      to_be_truthy: function(self)
      to_be_falsy: function(self)
      to_be_nil: function(self)
      to_not_be_nil: function(self)
      to_be_close: function(self, expected: number, eps?: number)
      to_be_greater_than: function(self, n: number)
      to_be_less_than: function(self, n: number)
      to_be_at_least: function(self, n: number)
      to_be_at_most: function(self, n: number)
      -- substring of a string (plain), or an element of an array
      to_contain: function(self, x: any)
      to_not_contain: function(self, x: any)
      -- Lua pattern on a string
      to_match: function(self, pattern: string)
      to_not_match: function(self, pattern: string)
      to_have_length: function(self, n: integer)
      to_error: function(self, pattern?: string)
      -- compare with tests/__snapshots__/<file>/<name>.snap; first run writes it,
      -- `htl test --update` rewrites a differing one
      to_match_snapshot: function(self, name: string)
   end

   record SnapshotConfig
      snapshot_dir: string
      update: boolean
      mkdir: function(string)
   end

   -- `t.expect_all(f()):to_equal(false, "no door")` for a function returning two values.
   record Expect2<A, B>
      to_equal: function(self, a: A, b: B)
   end

   record TestResult
      name: string
      ok: boolean
      ms: number
   end

   record Report
      passed: integer
      failed: integer
      failures: {string}
      tests: {TestResult}
      snapshots_written: {string}
      snapshots_updated: {string}
   end

   record RunOptions
      fail_fast: boolean
   end

   describe: function(name: string, body: function())
   it: function(name: string, body: function())
   expect: function<T>(actual: T): Expect<T>
   expect_all: function<A, B>(a: A, b: B): Expect2<A, B>
   run: function(filter?: string, opts?: RunOptions): Report
   count: function(): integer
   configure: function(SnapshotConfig)
   --- The run's random stream, shaped like `math.random`. Repeats under
   --- `htl test --seed <n>`; see the Tests section of the README.
   rng: function(): function(integer, integer): integer
end

return test
