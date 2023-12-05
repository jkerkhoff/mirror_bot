Tool that mirrors questions and resolutions from other forecasting platforms to [Manifold](https://manifold.markets/).


## Managram commands
People can interact with the bot by sending managrams. Managrams are checked every minute.

### [`mirror`](https://manifold.markets/mirrorbot?tab=managrams&a=60&msg=mirror%20http%3A%2F%2Fexample.com%2Fquestion)
To request a mirror for a specific question, [send a managram](https://manifold.markets/mirrorbot?tab=managrams&a=60&msg=mirror%20http%3A%2F%2Fexample.com%2Fquestion) for at least 60 mana with message `mirror <url>`, where `<url>` is a link to the original question. Currently this only supports Metaculus.

### [`resolve`](https://manifold.markets/mirrorbot?tab=payments&a=60&msg=resolve%20https%3A%2F%2Fmanifold.markets%2Fmirrorbot%2Fexample)
If a source question has resolved, you can request this resolution be applied to the mirror immediately by [sending a managram](https://manifold.markets/mirrorbot?tab=payments&a=60&msg=resolve%20https%3A%2F%2Fmanifold.markets%2Fmirrorbot%2Fexample) for any amount with message `resolve <url>`, where `<url>` is a link to the mirror market on Manifold.

### [`ping`](https://manifold.markets/mirrorbot?tab=managrams&a=10&msg=ping)
This just immediately returns the amount you sent. Might be useful to test if the bot is running.


## Source platforms

Supported:
- Metaculus
- Kalshi (no managrams yet)

Planned:
- Polymarket