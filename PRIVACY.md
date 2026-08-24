# Privacy Policy

**Effective 24 August 2026.**

This policy explains what VRCResolver collects, why, and how long it is kept. It covers
vrcresolver.com, the resolver and media proxy API, the Popcorn and Archive browsing features, the
peer relay, and the VRCResolver desktop client. The operator is WhyKnot. Contact:
**contact@whyknot.dev**.

The short version, before the detail:

- There are no accounts. You never give us a name, an email address, or a password.
- There is no advertising, no analytics, no tracking pixels, and no third-party trackers.
- We do not sell your personal information, and we do not share it for cross-context behavioural
  advertising. Both of those terms are used in the sense the California Consumer Privacy Act gives
  them.
- Your resolve history never leaves your browser.

## 1. What we collect automatically

<a id="s1"></a>

Running a web service means handling requests, and requests carry information whether or not anyone
wants them to:

- **Your IP address.** Read from the `X-Forwarded-For` header, which our proxy layer populates from
  Cloudflare's `CF-Connecting-IP`. It is used to apply rate limits, to identify and block abuse, and
  it appears in server logs.
- **Ordinary request metadata.** Timestamp, path, method, response status, response size, user
  agent, and referrer.
- **The URLs you submit.** A URL is the whole point of a resolve request, so we necessarily receive
  it. It may appear in application logs alongside the target player and the maximum height
  requested.
- **Correlation identifiers.** Random per-request IDs used to tie the stages of one resolve together
  when something fails. They are not tied to you and are not reused.
- **Approximate location.** Cloudflare, which sits in front of the Service, infers a country from
  your IP address as part of routing and abuse prevention.

We do not use cookies, fingerprinting, or any other technique to build a profile of you, and we do
not link requests across sessions.

## 2. What the desktop client sends

<a id="s2"></a>

The desktop client opens a WebSocket to the resolver and sends:

- the URL to resolve, the target player (`avpro` or `unity`), and a maximum height;
- occasional playback feedback after a video plays or fails, so failure patterns can be spotted;
- its version, so the server knows which protocol features it supports.

Playback feedback and failure reports are deliberately narrow. A report carries a domain, a failure
kind drawn from a fixed list, and a player name. Nothing else. The server actively rejects any
report containing something that looks like a file path, a home directory, or an IP address, rather
than quietly stripping it, so that if a future client version ever started leaking, the reports
would fail loudly instead of succeeding quietly.

The client keeps its own logs on your machine, in `%LOCALAPPDATA%Low\vrcresolver\logs\`. Those are
local. Nothing reads them but you, and the uninstaller deletes them.

## 3. What stays on your device

<a id="s3"></a>

These live in your browser's local storage and are never sent to us:

- your resolve history on the website, including titles and URLs;
- player volume and playback rate;
- a mesh welcome hash, used to avoid re-sending configuration the client already has;
- the Popcorn access value.

Clearing site data for vrcresolver.com removes all of it. On the desktop side, the uninstaller wipes
`%LOCALAPPDATA%Low\vrcresolver\`.

## 4. Cookies

<a id="s4"></a>

One cookie, for the Popcorn access gate. It stores the access value you entered, lasts 30 days, and
is set with `SameSite=Strict`. It is strictly necessary: without it you would re-enter the value on
every page load.

There are no analytics cookies, no advertising cookies, and no third-party cookies. That is also why
there is no consent banner, since there is nothing non-essential to consent to.

## 5. Why we process this, and on what basis

<a id="s5"></a>

- **Running the Service.** Resolving links, proxying media, and delivering streams cannot happen
  without receiving the request.
- **Abuse prevention and rate limiting.** IP addresses are the only workable signal when there are
  no accounts.
- **Diagnosis.** Logs are how a broken resolve gets fixed. Most bugs are visible only in the
  sequence of one failed request.
- **Capacity planning and security.**

For users in the European Economic Area and the United Kingdom, our legal basis for logging,
security, and abuse prevention is legitimate interests under Article 6(1)(f) of the GDPR, being our
interest in keeping a free service running and available. The Popcorn cookie is strictly necessary
for a service you asked for. We do not process special-category data, and we do not carry out
automated decision-making with legal or similarly significant effects. Rate limiting is applied to
an address, not to a person, and it restricts request volume rather than making any judgement about
you.

## 6. How long it is kept

<a id="s6"></a>

- **Application logs.** The active log file rotates after 7 days. Rotated files are compressed and
  deleted after 30 days. After that the entries are gone, including the IP addresses and URLs in
  them.
- **Caches.** Short-lived and technical. A resolved URL is cached for about 30 seconds, manifest
  bodies for about 20 seconds, and streaming session state for an hour or two. Transcoded segments
  are written to disk only while a stream is being watched, and are evicted under disk pressure.
- **Relayed files.** Never stored. The relay streams chunks out of the sharing browser on demand and
  forwards them; nothing is written to server storage, and closing the tab ends the share.

## 7. Who we disclose it to

<a id="s7"></a>

We do not sell personal information. We do not share it for advertising. There are no data brokers,
no advertising partners, and no analytics vendors.

Data is handled by the infrastructure that carries the traffic: our hosting providers, and
Cloudflare as the network and security layer in front of the Service. They process it to deliver the
traffic, not for their own purposes.

We will disclose information where we are legally required to, and where we believe in good faith it
is necessary to protect the Service, our rights, or someone's safety.

Worth stating because it works the other way from most services: when the proxy fetches a video, the
upstream host sees our server's address, not yours. Proxying your traffic is what the Service is
for, and reducing your exposure to upstream hosts is a side effect we consider a feature.

## 8. Third parties your browser contacts directly

<a id="s8"></a>

Some features are not proxied, which means those services receive your IP address directly and their
own policies apply:

- **The Internet Archive (archive.org).** Archive browsing, search, metadata, thumbnails, and
  downloads are fetched by your browser. See
  [archive.org/about/terms](https://archive.org/about/terms.php).
- **Popcorn (vr-m.net).** Catalogue queries reach a third-party service.
- **Cloudflare.** Fronts the Service. See
  [cloudflare.com/privacypolicy](https://www.cloudflare.com/privacypolicy/).

## 9. Your rights

<a id="s9"></a>

Depending on where you live, you may have the right to access, correct, delete, or receive a copy of
your personal information, to object to or restrict processing, and to complain to a data protection
authority. Where the CCPA applies, you also have the right to know, to delete, to correct, and to
opt out of sale or sharing. We do not sell or share, so there is nothing to opt out of, and we will
never discriminate against you for exercising a right.

Email contact@whyknot.dev to exercise any of these.

One honest caveat about what we can actually do. Without accounts, nothing we hold is tied to an
identity. If you ask for a copy of your data, the truthful answer is usually that we cannot find it,
because the only key is an IP address you would have to tell us, and matching on that alone would
mean handing you log entries that might belong to someone else who shared that address. Where we can
identify records, we act. Where we cannot, we will say so rather than pretend otherwise. In practice
the retention schedule in section 6 does the work: everything ages out within 30 days regardless.

## 10. Where processing happens, children, and security

<a id="s10"></a>

**Location.** The Service runs on two nodes, one serving traffic routed as United States and rest of
world, and one serving traffic routed as European. Using the Service means data is sent to whichever
node serves you, and between the two where they coordinate, which may be an international transfer.
We will name the hosting locations on request at contact@whyknot.dev.

**Children.** The Service is not directed at children under 13, and we do not knowingly collect
their personal information. If you believe a child has provided personal information, email
contact@whyknot.dev and we will delete it.

**Security.** We use HTTPS throughout, keep logs on access-restricted hosts, and hold the minimum
that makes the Service work. No system is completely secure, and we cannot guarantee absolute
security.

## 11. Changes and contact

<a id="s11"></a>

We may update this policy. The effective date at the top changes when we do, and the current version
is always at https://vrcresolver.com/privacy. Material changes will be noted there. Continued use
after a change means you accept it.

For personal-data requests and anything involving your own information, email
**contact@whyknot.dev**. Please do not put personal information in a public issue. For bugs,
questions, and anything that is not about your own data, the issue tracker at
https://github.com/RealWhyKnot/VRCResolver is usually faster.

See also the [Terms of Service](TERMS.md).
