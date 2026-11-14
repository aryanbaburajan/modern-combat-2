# Game files and multiplayer setup

## Files you need

Bring your own copy of **Modern Combat 2: Black Pegasus for Android, version 1.0.5**. The app expects two separate files:

- The game's **APK**. It must contain `lib/armeabi-v7a/libsandstorm2.so`.
- The game's **data ZIP**. It must contain the `GloftBPHP/` folder at the root of the ZIP.

If you received an outer archive containing both files, extract the outer archive first. Select the APK and the **inner data ZIP** on the startup screen; do not select the outer archive. The app does not provide the game files.

## Play multiplayer

1. Each player opens the same room link. For example, open `https://mc2.aryanbaburajan.com/?room=my-room-name` and share that exact link with the other players.
2. Each player imports their own APK and data ZIP if prompted, and starts the game. Use a different nickname for each player.
3. In the game, select **Multiplayer → Local Wi-Fi**. One player creates a match; the others search for it and join.

The room indicator at the top right shows connected browsers, which may not all have joined the match yet. Room names accept 1–64 letters, numbers, underscores, and hyphens. Anyone who can reach the relay and knows the room name can join, so use a long, hard-to-guess name for a private game. If the relay disconnects, reload to reconnect.

To play alone, open the site without a `room` parameter. Players on the deployed site use the Cloudflare relay; no one needs to run a local server. For local development, `npm run serve` also runs a relay on the same server, and every player must connect to that server's address with the same room name.
