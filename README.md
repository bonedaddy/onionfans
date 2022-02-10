# onionfans

Onlyfans-type web service based on TOR with maximum privacy features.

## Features

- "Vanishing" single-use feed CDN links
- Landing page
- No JavaScript!
- Auto-generated deposit addresses for users
- Minimum balances to enter and view "the feed"
- Monthly collection of user funds into a specified `WALLET_ADDRESS`
- `.mov` and `.jpg` content distribution on a feed

Note: This repository does not implement any TOR servicing. Set these up
yourself with a systemd unit.

## Usage

Edit the files in `static` and change the respective branding materials: `Store Title`, `Tagline`, etc...

Pass these environment variables when running:
- `WALLET_ADDRESS`: This is where your users' funds will be aggregated at the end of the month
- `ADMIN_PASS`: A secure admin password for testing purposes
- `CONTENT_FOLDER`: A local directory storing `.jpg` and `.mov` content

`WALLET_ADDRESS={} ADMIN_PASS={} CONTENT_FOLDER={} cargo run`
