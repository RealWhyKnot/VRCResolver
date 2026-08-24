# Terms of Service

**Effective 24 August 2026.**

These terms are a binding agreement between you and WhyKnot ("we", "us", the "Operator"), covering
VRCResolver and everything that runs on vrcresolver.com. Read them before you use the service. If
you do not accept them, do not use it.

Nothing here is legal advice, and none of it changes rights you hold under law that cannot be
signed away.

## 1. What this covers, and who can use it

<a id="s1"></a>

"Service" means the vrcresolver.com website and its subdomains, the resolver and media proxy API,
the Popcorn and Archive browsing features, the peer relay, the VRCResolver desktop application, and
the patched `yt-dlp` shim the desktop application installs. "You" means whoever uses any of it.

Using the Service means you accept these terms. That includes automated use by the desktop client
running on your machine, which acts on your behalf.

You must be at least 13 years old. If you are under 18, you may use the Service only with the
consent of a parent or guardian, who accepts these terms with you. If you use the Service on behalf
of an organisation, you warrant that you can bind it.

The desktop client is separately licensed under the GPL-3.0-or-later. That licence governs the
software itself: your right to run, study, modify, and redistribute the code. These terms govern the
hosted service the client talks to. Where the two overlap, the licence wins on questions about the
code and these terms win on questions about the service.

## 2. We carry traffic. We do not publish it

<a id="s2"></a>

This is the most important section in the document, so it comes early.

The Service is a conduit. When you paste a link, an automated system fetches what that link points
at and passes it back to you. Transmission is automatic, technical, passive, and transient. We do
not choose the material, we do not choose who receives it, we do not modify its substance beyond the
format changes needed to make it playable, and no person reviews it at any point.

We have no advance knowledge of what any URL resolves to. We do not maintain a catalogue of what has
passed through, and we do not hold copies beyond the short-lived technical caches described in the
[Privacy Policy](PRIVACY.md).

It follows that:

- The Service being able to resolve a link is not an endorsement of it, a representation that it is
  lawful, or a warranty that you are entitled to it.
- We make no representation about the accuracy, legality, safety, or quality of anything that passes
  through.
- Responsibility for what you request, transmit, or receive rests with you, in full.

We do not host content. We do not circumvent any technological protection measure, we do not strip
DRM, and we do not provide the means to do either. Any use of the Service to attempt that is a
breach of these terms.

## 3. Your content and your warranties

<a id="s3"></a>

Every time you submit a URL, share a file over the relay, or otherwise direct the Service to fetch
something, you warrant that:

- you own the material or hold the rights and licences needed to do what you are asking for;
- your request is lawful where you are, and would be lawful where our servers sit;
- your request does not breach the terms of service of the platform it points at, and that you have
  read them if you are unsure;
- the material is not unlawful, infringing, defamatory, or otherwise something we would be exposed
  to for carrying.

You keep whatever rights you already had in your own material. We claim none. You grant us only the
narrow, non-exclusive, worldwide, royalty-free licence needed to receive, transcode, cache, and
transmit it for the purpose of delivering it to you or to the recipient you chose, and that licence
ends when delivery ends.

Files shared over the peer relay are streamed from your own browser, in chunks, on demand. They are
not stored on our servers. When you close the tab, the share stops working.

## 4. Third-party services and content

<a id="s4"></a>

The Service reaches other people's systems, and in some cases your browser reaches them directly:

- **Upstream media hosts.** Whatever the URL you paste points at. Their terms apply to their
  material.
- **The Internet Archive.** Archive browsing queries archive.org from your own browser. We do not
  proxy it, we do not host it, and archive.org's terms and privacy policy apply to that traffic.
- **Popcorn (vr-m.net).** A third-party catalogue we query on your behalf. We neither operate it nor
  control what it returns.
- **Cloudflare.** Sits in front of the Service as a network and security layer.

We do not control these services. We do not warrant their availability, accuracy, or lawfulness, and
we accept no liability for them. Links and integrations are not endorsements. If a third party
changes, breaks, or withdraws, the corresponding part of the Service may stop working with no notice
and no remedy.

## 5. No affiliation

<a id="s5"></a>

VRCResolver is an independent project. It is not affiliated with, endorsed by, sponsored by, or
approved by VRChat Inc., Google LLC, YouTube, the Internet Archive, SoundCloud, Twitch, or any other
platform it can resolve links from. All trademarks belong to their owners and are used only to
describe what the software interoperates with.

Your use of any third-party platform remains governed by your agreement with that platform. Using
VRCResolver does not change that agreement, and does not give you rights you would not otherwise
have. If a platform's terms prohibit what you are doing, the fact that our software made it
technically possible is not a defence, and it is your problem rather than ours.

## 6. Acceptable use

<a id="s6"></a>

You may not:

- access the Service by automated means other than the desktop client and the website as published,
  including scripts, bots, headless browsers, scrapers, or reimplementations of the protocol;
- mirror, scrape, or bulk enumerate the Service, its API, or anything reachable through it;
- resell, sublicense, rent, or otherwise commercially redistribute access or capacity, whether or
  not you charge for it;
- use the Service as backend infrastructure for another product, site, bot, or service;
- probe, test, or work around rate limits, quotas, or access controls, including by rotating IP
  addresses, using proxies or VPNs to evade a block, or splitting load across identities;
- evade, or attempt to evade, any block, ban, or restriction we have applied;
- use the Service to obtain, transmit, or distribute material that is unlawful, that infringes
  someone else's rights, or that constitutes child sexual abuse material, which we report and act on
  without exception;
- interfere with, disrupt, or place disproportionate load on the Service or the networks it depends
  on, or attempt to gain unauthorised access to any part of it;
- remove, obscure, or alter attribution, licence text, or notices in the client;
- misrepresent your traffic as something it is not, including by spoofing headers or forging client
  identifiers.

## 7. Excessive use

<a id="s7"></a>

The Service is free, and capacity is finite and shared. It is sized for ordinary personal use:
resolving links you are about to watch, in a world you are actually in.

Rate limits apply per address across the resolver, the media proxy, the Popcorn endpoints, and the
relay, and we may enforce them at any time. **We do not publish the thresholds.** Publishing them would tell abusive
users exactly where to sit, and would tie us to numbers we adjust as load changes. They may change
at any time without notice.

Whether your use is excessive is determined by us, acting in our sole discretion. Sustained volume
disproportionate to ordinary personal use is excessive whether or not it trips a limit, and whether
or not any individual request was permitted. Requests that are rejected for exceeding a limit are
still requests: repeatedly hammering a limit is itself excessive use.

We are not obliged to warn you before treating your use as excessive, and a period of tolerated
heavy use creates no entitlement to continue.

## 8. Enforcement, blocking, and termination

<a id="s8"></a>

We may throttle, suspend, block, or permanently terminate access for any user, IP address, address
range, network, or client installation, at any time, with or without notice, with or without cause,
in our sole and absolute discretion. Breach of these terms is sufficient reason. So is a good-faith
belief that a breach is likely, or that continued access risks harm to the Service, to us, or to
anyone else.

We are under no obligation to tell you that you have been blocked, to explain why, to identify the
conduct involved, to give you an opportunity to fix it, to preserve any data, or to offer an appeal
or review. Any of these we may do as a courtesy, and doing it once creates no expectation that we
will do it again.

Because the Service is free, no refund, credit, or compensation arises on suspension or termination.

Attempting to evade a block, by any means, is a further and separate breach of these terms, and may
result in blocks applied more broadly than the conduct that caused the first one.

Sections 2 through 5 and 9 through 14 survive termination.

## 9. Copyright complaints and takedown

<a id="s9"></a>

If you believe material reachable through the Service infringes your copyright, email
**contact@whyknot.dev** with:

1. identification of the copyrighted work you say has been infringed;
2. identification of the material complained of, and enough detail to locate it, meaning the exact
   URL or request that reaches it;
3. your name, address, telephone number, and email address;
4. a statement that you have a good-faith belief that the use is not authorised by the copyright
   owner, its agent, or the law;
5. a statement that the information in your notice is accurate, and, under penalty of perjury, that
   you are the owner or are authorised to act on the owner's behalf;
6. your physical or electronic signature.

On receiving a notice that is complete on its face, we will act promptly to block access through the
Service. Because we host nothing, this usually means blocking a URL, a pattern, or a domain rather
than deleting a file. We terminate access for users who repeatedly direct the Service at infringing
material.

**We do not claim safe harbour under 17 U.S.C. 512, and we have not designated an agent under that
section.** This procedure is voluntary. Operating it is not an admission that we are a service
provider within the meaning of that section, that we have any obligation under it, or that any
material was infringing.

Notices that are incomplete, abusive, or sent in bad faith may be ignored. Misrepresenting that
material is infringing can make you liable for damages under applicable law.

## 10. Availability

<a id="s10"></a>

The Service is provided free of charge. There is no service level agreement, no uptime commitment,
no support obligation, and no promise that any feature will continue to exist.

We may change, suspend, limit, or discontinue the Service or any part of it, at any time, for any
reason, without notice and without liability. Features may be removed. Behaviour may change without
warning, including in ways that break how you were using it. Upstream platforms change constantly,
and things that worked yesterday may not work today.

## 11. Disclaimer of warranties

<a id="s11"></a>

THE SERVICE IS PROVIDED "AS IS" AND "AS AVAILABLE", WITH ALL FAULTS AND WITHOUT WARRANTY OF ANY
KIND.

To the fullest extent permitted by law, we disclaim all warranties, express, implied, and statutory,
including the implied warranties of merchantability, fitness for a particular purpose, title,
non-infringement, quiet enjoyment, and accuracy of data, and any warranty arising from course of
dealing, course of performance, or usage of trade.

We do not warrant that the Service will be uninterrupted, timely, secure, or error-free, that
defects will be corrected, that any link will resolve, that any stream will play, or that the
Service or the servers that run it are free of harmful components. No advice or information you get
from us, in any form, creates a warranty not stated here.

The desktop client carries its own warranty disclaimer under the GPL-3.0-or-later. This section
supplements it and does not replace it.

Some jurisdictions do not allow the exclusion of certain warranties. Where that is so, the
exclusions above apply to the maximum extent that jurisdiction permits, and nothing here affects
non-excludable statutory or consumer rights you hold.

## 12. Limitation of liability

<a id="s12"></a>

To the fullest extent permitted by law, we will not be liable for any indirect, incidental, special,
consequential, exemplary, or punitive damages, or for any loss of profits, revenue, data, goodwill,
or anticipated savings, or for the cost of substitute services, arising out of or relating to the
Service or these terms, however caused and on any theory of liability, whether in contract, tort
(including negligence), strict liability, or otherwise, and even if we have been advised of the
possibility.

Our total aggregate liability for all claims relating to the Service is limited to the greater of
the amount you actually paid us in the twelve months before the claim arose, which for a free
service is zero, or fifty United States dollars (USD 50).

These limits apply even if a limited remedy fails of its essential purpose. They are a fundamental
basis of the bargain between us: we could not offer the Service free of charge without them.

Some jurisdictions do not allow the exclusion or limitation of certain damages. Where that is so,
our liability is limited to the smallest amount that jurisdiction permits, and nothing in this
section excludes liability for death or personal injury caused by negligence, for fraud, or for
anything else that cannot lawfully be excluded.

## 13. Indemnification

<a id="s13"></a>

You will indemnify, defend, and hold harmless the Operator, and its contributors, agents, and
service providers, against all claims, demands, actions, damages, losses, liabilities, penalties,
costs, and expenses, including reasonable legal fees, arising out of or relating to:

- your use of the Service;
- any URL you submit, any file you relay, and any material you transmit or receive;
- your breach of these terms or of any warranty you gave in them;
- your violation of any law or of any right of any third party, including intellectual property,
  privacy, and publicity rights.

We will notify you of any claim we seek indemnity for, and you will not settle any claim in a way
that imposes an obligation or admission on us without our written consent. We reserve the right to
assume the exclusive defence and control of any matter subject to indemnification by you, at your
expense, and you will cooperate with us if we do.

## 14. Disputes and general terms

<a id="s14"></a>

**Governing law.** These terms are governed by the laws of the United States and of the state in
which the Operator resides, without regard to conflict-of-laws rules, and without regard to the UN
Convention on Contracts for the International Sale of Goods. We will identify that state on request
at contact@whyknot.dev.

**Talk to us first.** Before filing anything, email contact@whyknot.dev with a description of the
dispute and what you want. Most things are fixable in a message. Neither of us may start proceedings
until 30 days after that email, unless a claim needs urgent injunctive relief.

**Venue.** The state and federal courts serving the Operator's place of residence have exclusive
jurisdiction over any dispute that is not resolved informally. You and we each consent to personal
jurisdiction there and waive any objection based on venue or forum non conveniens.

**Jury trial waiver.** To the extent permitted by law, you and we each waive the right to a trial by
jury.

**No class actions.** To the extent permitted by law, claims may be brought only in an individual
capacity. You and we each waive the right to bring or participate in a class, collective,
consolidated, or representative action. No arbitrator or court may consolidate claims without the
written consent of both of us.

**Time limit.** Any claim relating to the Service must be brought within one year after it arose, or
it is permanently barred, except where a longer period cannot be waived by law.

**Changes.** We may change these terms. The effective date at the top will change, and the current
version is always at https://vrcresolver.com/terms. Continued use after a change means you accept
it. If you do not accept a change, stop using the Service.

**Severability.** If any provision is held unenforceable, it is modified to the minimum extent
needed to make it enforceable, or severed if it cannot be, and the rest stays in force.

**No waiver.** Not enforcing a provision is not a waiver of it. A waiver is only effective if it is
in writing.

**Assignment.** You may not assign or transfer these terms or any rights under them. We may assign
them freely, including as part of a transfer of the project.

**Force majeure.** We are not liable for any failure or delay caused by anything beyond our
reasonable control, including outages at hosting or network providers, upstream platform changes,
denial-of-service attacks, and acts of government.

**Entire agreement.** These terms and the [Privacy Policy](PRIVACY.md) are the entire agreement
between us about the Service, and supersede anything said before.

**Contact.** contact@whyknot.dev. For anything that is not a legal notice or a personal-data
request, the issue tracker at https://github.com/RealWhyKnot/VRCResolver is usually faster.
