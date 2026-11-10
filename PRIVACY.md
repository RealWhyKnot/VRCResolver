# Privacy policy

**Effective 10 November 2026.**

The live version is at [vrcresolver.com/privacy](https://vrcresolver.com/privacy). This copy is
updated automatically when the live version changes.

This policy covers what VRCResolver collects, why, and how long it's kept. It applies to
vrcresolver.com, the resolver and media proxy API, the peer relay, browser streaming, and the
VRCResolver desktop client. VRCResolver is run by WhyKnot, and you can reach us at
[contact@whyknot.dev](mailto:contact@whyknot.dev).

In short:

- There are no accounts. You don't give us a name, an email address or a password.
- We don't use advertising, analytics, tracking pixels or third-party trackers.
- We don't sell your personal information, and we don't share it for cross-context behavioural
  advertising. We mean both terms the way the California Consumer Privacy Act defines them.
- Your resolve history is stored only in your browser.

## 1. Data we collect automatically

<a id="s1"></a>

- **Your IP address.** We read it from the `X-Forwarded-For` header, which our proxy fills in from
  Cloudflare's `CF-Connecting-IP`. We use it for rate limits and to find and block abuse, and it
  shows up in server logs.
- **Ordinary request metadata.** Timestamp, path, method, response status, response size, user
  agent, and referrer.
- **The URLs you submit.** We can't resolve a URL without receiving it. It may show up in
  application logs along with the target player and the maximum height you asked for.
- **Correlation identifiers.** Random IDs that let us follow one resolve, and the playlist and
  segment requests of the playback it starts, through each stage when something fails. A new set is
  made for every resolve. They aren't linked to you and aren't reused.
- **Approximate location.** Cloudflare, which runs in front of the Service, works out a country from
  your IP address for routing and abuse prevention.

We don't use cookies, fingerprinting, or any other technique to build a profile of you, and we don't
link requests across sessions.

## 2. Data the desktop client sends

<a id="s2"></a>

The desktop client opens a WebSocket to the resolver and sends:

- the URL to resolve, the target player (`avpro` or `unity`), and a maximum height;
- occasional playback feedback after a video plays or fails, which we use to spot patterns in
  failures;
- its version, which tells the server what protocol features it supports.

A playback or failure report holds a domain, a failure type from a fixed list, and a player name. A
failure report also holds the random IDs of the request and playback that failed (the ones described
in section 1), whether the failure was on the client's side or the server's, the player's error
code, the last HTTP status the server returned, and how many repeated requests the client held back.
If any part of a report looks like a file path, a home directory or an IP address, the server
rejects the whole report with an error. That also covers a future client version that starts sending
that kind of data by mistake.

The client keeps its own logs on your machine, in `%LOCALAPPDATA%Low\vrcresolver\logs\`, and doesn't
send them to us. The uninstaller deletes them.

## 3. Data kept on your device

<a id="s3"></a>

Your browser keeps these in local storage and doesn't send them to us:

- your resolve history on the website, including titles and URLs;
- player volume and playback rate;
- a mesh welcome hash, used to avoid re-sending configuration the client already has.

Clearing site data for vrcresolver.com removes all of it. For the desktop client, the uninstaller
deletes `%LOCALAPPDATA%Low\vrcresolver\`.

## 4. Cookies

<a id="s4"></a>

We don't set any cookies, analytics, advertising and third-party ones included. That's why there's
no consent banner.

## 5. Why we process this, and on what basis

<a id="s5"></a>

- **Running the Service.** We can't resolve a link, proxy media or deliver a stream without
  receiving the request.
- **Abuse prevention and rate limiting.** Without accounts, an IP address is the only thing we can
  rate-limit or block by.
- **Diagnosis.** We fix broken resolves from the logs, since most bugs only show up in the steps of
  a single failed request.
- **Capacity planning and security.**

For users in the European Economic Area and the United Kingdom, our legal basis for logging,
security, and abuse prevention is legitimate interests under Article 6(1)(f) of the GDPR, namely our
interest in keeping a free service up and running. We don't process special-category data, and we
don't carry out automated decision-making with legal or similarly significant effects. A rate limit
only counts how many requests come from an IP address.

## 6. How long it's kept

<a id="s6"></a>

- **Application logs.** Logs are kept in the server's memory and encrypted every minute. The key
  that decrypts them isn't on the server. Until an hour is over, the server can still read that
  hour's logs; after that only the encrypted copy is left. We delete the encrypted logs after 30
  days, on the server and on our own copy, along with the IP addresses and URLs in them.
- **Caches.** Cached data is technical and short-lived. A resolved URL is cached for up to 15
  minutes, a URL with nothing playable for an hour, stream manifests for about 20 seconds, and
  streaming session state for up to two hours. Transcoded segments only go to disk while someone is
  watching the stream, and they're removed if the disk starts running out of space.
- **Browser streams.** These are kept in memory and aren't written to disk. While you're live, the
  server keeps the last 12 seconds or so of video for viewers who are catching up, and drops older
  segments as new ones come in. Whatever's left is dropped within a minute of the stream ending.
  Anyone with the link can watch while you're live.
- **Relayed files.** We don't store these. The relay pulls chunks from the sharing browser as
  they're requested and passes them on without saving them on the server. Closing the tab ends the
  share.

## 7. Who we disclose it to

<a id="s7"></a>

We don't sell personal information or share it for advertising. We don't work with data brokers,
advertising partners or analytics vendors.

Our hosting providers and Cloudflare, which handles the network and security in front of the
Service, see data as it passes through them. They only process it to deliver the traffic.

We'll disclose information where we're legally required to, and where we believe in good faith it's
necessary to protect the Service, our rights, or someone's safety.

When the proxy fetches a video, the site hosting it only sees our server's address.

If our server can't resolve a YouTube link, the desktop client asks YouTube for it from your PC, and
VRChat's own yt-dlp does the same if that fails too. In those cases YouTube sees your IP address, as
it would without VRCResolver.

## 8. Third parties your browser contacts directly

<a id="s8"></a>

Your browser connects to these directly. They receive your IP address, and their own privacy
policies apply.

- **Cloudflare.** Runs in front of the Service. See
  [cloudflare.com/privacypolicy](https://www.cloudflare.com/privacypolicy/).

## 9. Your rights

<a id="s9"></a>

Depending on where you live, you may have the right to access, correct, delete, or receive a copy of
your personal information, to object to or restrict processing, and to complain to a data protection
authority. Where the CCPA applies, you also have the right to know, to delete, to correct, and to
opt out of sale or sharing. We don't sell or share personal information. We won't discriminate
against you for using any of these rights.

Email [contact@whyknot.dev](mailto:contact@whyknot.dev) to exercise any of these.

Since there are no accounts, the data we hold isn't tied to a person, and if you ask for a copy of
yours we usually can't find any. The only thing we could search by is an IP address you give us, and
matching on that alone might turn up log entries from someone else who used the same address. If we
can identify your records, we'll act on your request, and if we can't, we'll tell you. Logs are
deleted after 30 days, as section 6 describes.

## 10. Where processing happens, children, and security

<a id="s10"></a>

**Location.** The Service runs on two servers, one for traffic routed to the United States and the
rest of the world, and one for traffic routed to Europe. Your data goes to whichever one serves you,
and between the two when they coordinate, which can count as an international transfer. Email
[contact@whyknot.dev](mailto:contact@whyknot.dev) if you'd like to know where they're hosted.

**Children.** The Service isn't directed at children under 13, and we don't knowingly collect their
personal information. If you think a child has given us personal information, email
contact@whyknot.dev and we'll delete it.

**Security.** Connections to the Service use HTTPS. We keep logs encrypted on servers with
restricted access and hold only what the Service needs to work. We can't promise that any system is
completely secure, ours included.

## 11. Changes and contact

<a id="s11"></a>

We may update this policy. When we do, the date at the top changes, and the current version is at
https://vrcresolver.com/privacy, where we'll point out any significant changes. If you keep using
the Service after a change, you're accepting it.

Email [contact@whyknot.dev](mailto:contact@whyknot.dev) about anything involving your own
information, and please don't put personal information in a public issue. For bugs and other
questions, the [issue tracker](https://github.com/RealWhyKnot/VRCResolver/issues) is usually faster.

See also the [Terms of Service](TERMS.md).
