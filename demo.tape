# demo.tape — charmbracelet/vhs script
# Record with: vhs demo.tape
# Output: demo.gif

Output demo.gif

Set Width 900
Set Height 600
Set FontSize 16
Set TypeSpeed 50ms
Set Theme "Catppuccin Mocha"

# Start a dummy HTTP server in the background
Shell "python3 -m http.server 3000 &>/dev/null &"
Sleep 500ms

# Launch localshare
Type "localshare 3000"
Enter
Sleep 4s

# Simulate incoming requests to populate the live request log.
# These curl calls hit the local server directly through the public URL.
# The relay-assigned subdomain is captured at record time — replace the
# placeholder below with the actual URL shown in the banner, or pipe
# localshare --json output to extract it automatically.
Shell "curl -s -o /dev/null http://127.0.0.1:3000/"
Sleep 800ms
Shell "curl -s -o /dev/null http://127.0.0.1:3000/index.html"
Sleep 800ms
Shell "curl -s -o /dev/null http://127.0.0.1:3000/favicon.ico"
Sleep 2s

# Show the graceful shutdown
Type "C-c"
Sleep 1s
