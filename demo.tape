# demo.tape — charmbracelet/vhs script
# Record with: vhs demo.tape
# Output: demo.gif

Output demo.gif

Set Width 1000
Set Height 650
Set FontSize 15
Set TypeSpeed 60ms
Set Theme "Catppuccin Mocha"
Set WindowBar Colorful
Set Padding 20

# 1. Setup background tasks silently
Hide
# Start a dummy HTTP server
Shell "python3 -m http.server 3000 &> /dev/null &"
# Schedule HTTP requests to the PUBLIC URL in the background to populate the log UI.
# This proves the public tunnel works without needing a valid local IP.
Shell "(sleep 5; curl -s -o /dev/null https://my-demo.relay.localshare.dev/; sleep 1.5; curl -s -X POST -o /dev/null https://my-demo.relay.localshare.dev/api/webhooks/stripe; sleep 1.5; curl -s -o /dev/null https://my-demo.relay.localshare.dev/favicon.ico) &"
Show

# 2. Launch localshare with a deterministic subdomain
Type "localshare 3000 -s my-demo"
Enter

# 3. Wait while the background curls hit the public URL and populate the UI
Sleep 10s

# 4. Show graceful shutdown
Ctrl+C
Sleep 2s
