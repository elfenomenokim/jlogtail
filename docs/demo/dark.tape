Output docs/demo/dark.gif
Set Shell bash
Set FontSize 18
Set Width 1200
Set Height 620
Set Padding 28
Set Theme "Catppuccin Mocha"
Set TypingSpeed 45ms
Set WindowBar Colorful
Set BorderRadius 10
Set Framerate 15
Hide
Set TypingSpeed 0
Sleep 500ms
Type "source docs/demo/setup.sh"
Enter
Sleep 500ms
Type "clear"
Enter
Sleep 600ms
Show
Set TypingSpeed 45ms
Type "jlogtail docs/demo/agents.jsonl --lines 2 --filter role=assistant --fields role,text"
Enter
Sleep 2s
