# Config errors quote nothing the operator wrote

A config error reaches the startup output, the log and the list page, and anyone who can open the list page can read it. Anything in the config may be a credential: a token pasted as a `hosts` key, a password in a repo URL, a token where a host belongs. So a config error is built only from text the app wrote, the names of its own settings, and toml's and serde's own words. Line and column say where the problem is, and a rejected `repos` entry is named by its position. It never quotes a URL, a host, a key or a value the operator wrote, not even one that looks harmless.

## Considered Options

- **Quote what looks safe**: repo URLs with their user info replaced by `…`, and `hosts` keys that look like domain names. We tried this, and every heuristic needed its own security argument and a follow-up fix: escaped `@`, `://` inside passwords, bare tokens shaped like hosts. One gap even survived the review: a token in a URL's host position (`https://ghp_…`) was quoted. Naming the text adds little, because line and column already find it.
- **Treat the log and the startup error as the operator's own, and hide text only on the list page**: rejected, because logs are often shipped to systems other people can read.
