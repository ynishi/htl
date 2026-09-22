local record mq
   record Color
      r: number
      g: number
      b: number
      a: number
   end
   record Vec2
      x: number
      y: number
   end
   enum Key
      "Space"
      "Escape"
      "Enter"
      "Tab"
      "Backspace"
      "Delete"
      "Insert"
      "Home"
      "End"
      "PageUp"
      "PageDown"
      "Up"
      "Down"
      "Left"
      "Right"
      "LeftShift"
      "RightShift"
      "LeftControl"
      "RightControl"
      "LeftAlt"
      "RightAlt"
      "A"
      "B"
      "C"
      "D"
      "E"
      "F"
      "G"
      "H"
      "I"
      "J"
      "K"
      "L"
      "M"
      "N"
      "O"
      "P"
      "Q"
      "R"
      "S"
      "T"
      "U"
      "V"
      "W"
      "X"
      "Y"
      "Z"
      "Key0"
      "Key1"
      "Key2"
      "Key3"
      "Key4"
      "Key5"
      "Key6"
      "Key7"
      "Key8"
      "Key9"
      "F1"
      "F2"
      "F3"
      "F4"
      "F5"
      "F6"
      "F7"
      "F8"
      "F9"
      "F10"
      "F11"
      "F12"
   end
   enum MouseButton
      "Left"
      "Middle"
      "Right"
   end
   screen_width: function(self: mq): number
   screen_height: function(self: mq): number
   frame_time: function(self: mq): number
   time: function(self: mq): number
   fps: function(self: mq): integer
   clear_background: function(self: mq, color: Color)
   draw_rectangle: function(self: mq, x: number, y: number, w: number, h: number, color: Color)
   draw_rectangle_lines: function(self: mq, x: number, y: number, w: number, h: number, thickness: number, color: Color)
   draw_circle: function(self: mq, x: number, y: number, r: number, color: Color)
   draw_circle_lines: function(self: mq, x: number, y: number, r: number, thickness: number, color: Color)
   draw_line: function(self: mq, x1: number, y1: number, x2: number, y2: number, thickness: number, color: Color)
   draw_text: function(self: mq, text: string, x: number, y: number, font_size: number, color: Color)
   is_key_down: function(self: mq, key: Key): boolean
   is_key_pressed: function(self: mq, key: Key): boolean
   is_key_released: function(self: mq, key: Key): boolean
   mouse_position: function(self: mq): Vec2
   is_mouse_button_down: function(self: mq, button: MouseButton): boolean
   is_mouse_button_pressed: function(self: mq, button: MouseButton): boolean
end

return mq
