# README demo. Render with scripts/demo/record.sh, not vhs directly: the
# script builds the fake world this tape drives and adds the key captions.
#
# "#cap <keys> | <what they do>" lines are captions: record.sh times them
# from the Sleep and key commands below and draws them under the terminal.
# A caption stays until the next one; "#cap -" clears it.
Output "/tmp/flockr-demo/demo.mp4"

Set Shell bash
Set FontFamily "JetBrainsMono Nerd Font"
Set FontSize 15
Set Width 1500
Set Height 880
Set Padding 12
Set Theme "TokyoNight"
Set Framerate 20
Set TypingSpeed 100ms

Hide
Type "/tmp/flockr-demo/start"
Enter
Sleep 2.5s
Show

#cap | four Claude agents working on web-app at once
Sleep 3s

#cap ctrl+b space | next layout
Ctrl+B
Space
Sleep 1.4s
Ctrl+B
Space
Sleep 1.4s
#cap ctrl+b alt+5 | tiled layout
Ctrl+B
# Alt+5 (VHS's Alt+ takes letters only; a terminal sends Alt as Escape first).
Type "5"
Sleep 1.2s

#cap | acme-api finished in the background
Sleep 2s

#cap ctrl+b a | jump to the agent that needs you
Ctrl+B
Type "a"
Sleep 2.2s
#cap 1 | approve the edit
Type "1"
Sleep 2s

#cap ctrl+b a | jump to the next one
Ctrl+B
Type "a"
Sleep 2.2s

#cap alt+4 | jump to agent 4
# Alt+4: back to web-app's first agent.
Type "4"
Sleep 1.2s
#cap ctrl+b f | label every link, type a label to open it
Ctrl+B
Type "f"
Sleep 2.2s
Escape
Sleep 0.4s

#cap ctrl+b : | command palette: search every action
Ctrl+B
Type ":"
Sleep 0.6s
Type "layout"
Sleep 1.8s
Escape
Sleep 0.4s

#cap ctrl+b s | settings
Ctrl+B
Type "s"
Sleep 1.2s
#cap tab | next section
Tab
Sleep 1.2s
#cap → | sidebar on the right: saved and applied at once
Right
Sleep 2.2s
#cap ← | back to the left
Left
Sleep 1.4s
#cap -
Escape
Sleep 1.5s
