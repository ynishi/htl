local record proc
   record Options
      cwd: string
      env: {string:string}
      stdin: string
      timeout_ms: integer
   end
   record Exit
      code: integer
      signal: integer
      stdout: string
      stderr: string
      timed_out: boolean
   end
   run: function(argv: {string}, opts?: Options): Exit ---@async
end

return proc
