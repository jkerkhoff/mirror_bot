Tool that mirrors questions and resolutions from other forecasting platforms to [Manifold](https://manifold.markets/).


## Managram commands
People can interact with the bot by sending managrams. Managrams are checked every minute.

### [`mirror`](https://manifold.markets/mirrorbot?tab=managrams&a=60&msg=mirror%20http%3A%2F%2Fexample.com%2Fquestion)
To request a mirror for a specific question, [send a managram](https://manifold.markets/mirrorbot?tab=managrams&a=60&msg=mirror%20http%3A%2F%2Fexample.com%2Fquestion) for at least 60 mana with message `mirror <url>`, where `<url>` is a link to the original question. Currently this only supports Metaculus.

### [`ping`](https://manifold.markets/mirrorbot?tab=managrams&a=10&msg=ping)
This just immediately returns the amount you sent. Might be useful to test if the bot is running.


## Source platforms

Supported:
- Metaculus
- Kalshi (no managrams yet)

Planned:
- Polymarket