local record {{mod}}
   record Greeting
      who: string
      text: string
   end
end

function {{mod}}.greet(who: string): {{mod}}.Greeting
   return { who = who, text = "hello, " .. who }
end

return {{mod}}
