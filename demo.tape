# demo.tape — charmbracelet/vhs script
# Record with: vhs demo.tape
# Output: demo.gif

Output demo.gif

Set Width 1000
Set Height 650
Set FontSize 15
Set Theme "Catppuccin Mocha"
Set WindowBar Colorful
Set Padding 20

# Enable offline mock mode — no internet or relay required
Env LOCALSHARE_DEMO_MODE "1"

Type "localshare 3000 -s my-demo"
Enter

# The mock mode emits Connected + 3 request logs automatically
Sleep 7s

# Show graceful shutdown
Ctrl+C
Sleep 2s
