# README demo. Render with scripts/demo/record.sh, not vhs directly: the
# script builds the fake world this tape drives.
Output assets/demo.gif

Set Shell bash
Set FontFamily "JetBrainsMono Nerd Font"
Set FontSize 15
Set Width 1500
Set Height 880
Set Padding 12
Set Theme "TokyoNight"
Set Framerate 20
Set TypingSpeed 120ms

Hide
Type "/tmp/flockr-demo/start"
Enter
Sleep 2.5s
Show

# The overview: spaces with branches and changes, worktrees nested under
# acme-api, and agents working, blocked, and done.
Sleep 2.5s

# Layout presets cycle the web-app panes.
Ctrl+B
Space
Sleep 1.2s
Ctrl+B
Space
Sleep 1.2s
Ctrl+B
# Alt+1 (VHS's Alt+ takes letters only; a terminal sends Alt as ESC first).
Type "1"
Sleep 1s

# acme-api finishes in the background and raises a toast.
Sleep 1.5s

# Jump to the agent that needs you and approve its edit.
Ctrl+B
Type "a"
Sleep 2.2s
Type "1"
Sleep 2s

# Jump to the next one: the agent that just finished.
Ctrl+B
Type "a"
Sleep 2.2s

# Link hints label every link on screen.
# Alt+4: focus agent 4, back to web-app.
Type "4"
Sleep 0.6s
# Focus the dev server pane to the right; hints cover the focused pane.
Ctrl+B
Type "l"
Sleep 0.6s
Ctrl+B
Type "f"
Sleep 2s
Escape
Sleep 0.5s

# The command palette searches every action.
Ctrl+B
Type ":"
Sleep 0.6s
Type "layout"
Sleep 1.8s
Escape
Sleep 0.5s

# Settings save to the config file and apply at once.
Ctrl+B
Type "s"
Sleep 1.2s
Tab
Sleep 1s
Right
Sleep 2s
Left
Sleep 1.2s
Escape
Sleep 1.5s
