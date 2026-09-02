# demo.tape — charmbracelet/vhs script
# Record with: vhs demo.tape
# Output: demo.gif

Output demo.gif

# Set terminal dimensions for a clean, compact recording
Set Width 90
Set Height 28
Set FontSize 14

# Start a dummy local HTTP server in the background
Shell "python3 -m http.server 3000 &>/dev/null &"
Sleep 500ms

# Launch localshare against the dummy server
Type "localshare 3000"
Enter

# Wait for the banner + QR code to fully render
Sleep 3s

# Open a new pane and curl the public URL to trigger a 200 OK in the live log
# We use the URL from the banner — replace with the actual public URL on record
# The grep + sed extracts the URL dynamically from localshare's JSON output
# Alternatively, hardcode a known URL if using a persistent relay
Split
Type "curl -s -o /dev/null -w '%{http_code}' https://REPLACE_WITH_PUBLIC_URL"
Enter
Sleep 2s

# Switch back to the main pane to show the live request log
Focus
Sleep 2s

# Demonstrate graceful shutdown
Type "C-c"
Sleep 1s
