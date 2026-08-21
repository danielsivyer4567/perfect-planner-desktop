# Collision Assessor guarantee and trust boundary

Status: normative design contract for PP-002. The implementation and release tests must fail
closed when they cannot prove this contract.

## The guarantee

Perfect Planner guarantees that no **managed worker** receives or renews an edit claim unless
all of the following facts are true at the same instant:

1. An independent collision assessor has completed the configured machine-wide Planner census.
2. Every registered Planner, configured discovery root, plan manifest, active lease and declared
   resource is accounted for and fresh.
3. The assessor verdict is `CLEAR`; `WAIT`, `REPLAN`, `USER_DECISION` and `UNKNOWN` never grant a
   claim.
4. A live clearance is bound to the exact run, node, logical repository, branch, normalized file
   manifest, resource manifest, registry generation, worker fence and immutable snapshot hash.
5. The single-use discovery capability has been consumed and revoked.
6. The user approval is bound to exactly one registered originating chat, and a durable delivery
receipt proves that the approval notification reached that chat.
7. The scheduler atomically consumes the clearance before returning the worker lease.

The originating-chat delivery receipt is therefore a mandatory admission input, not an
informational notification.

The enforcement point is worker admission. A UI label, a JSON value saying `approved`, a chat
message, a model statement, or an assessor process merely being present is never sufficient.

## Census authority and first-census bootstrap

The machine-wide registry and the plan files named by its live registrations are the census
authority. Board HTTP is an untrusted display transport: `/whoami`, `/plan` or any other loopback
response may help render status, but it cannot add a Planner, replace a plan manifest or satisfy
an absent registry fact.

The first census does not require a previous census. Under the cross-process registry lock, the
native assessor loads the registry once, validates its schema and strict structural bounds,
rejects stale or future-dated registrations, and binds every configured root, repository root,
worktree root and plan file to the operating system's volume and file identity. Only then does it
issue a private first-census snapshot. Missing, malformed, oversized, aliased or unresolvable
authority yields `UNKNOWN`; it never yields an empty snapshot.

The snapshot carries a domain-separated SHA-256 digest over the registry schema and generation,
sorted configured-root identities, sorted registration authority, lease generations, canonical
node/file/resource manifests, and physical root/plan identities. Registry update timestamps and
prior census output are deliberately excluded, so merely recording a census cannot invalidate
its own authority input. Freshness is validated separately both when the snapshot is issued and
when its result is consumed.

Census output is persisted through one conditional operation. It reacquires the registry lock,
rebuilds the current authority snapshot, compares both generation and digest, revalidates every
physical identity and writes only if the input is unchanged. A concurrent heartbeat, manifest
mutation, root change, delete/recreate, mapping change or stale duplicate result is rejected.

On Windows, stable local volume/file IDs are equality authority; path spelling is not. Two stable
hardlinks therefore identify the same object and must collide. Junctions, symbolic links, reparse
points, SUBST drives, mapped/remote drives, unsupported or zero file IDs, and identities that
change during validation force `UNKNOWN` instead of falling back to lower-cased path text.

## Three separate authorities

The system deliberately separates authority:

- The **collision assessor observes**. It can read bounded Planner registration and manifest
  metadata during its one-use discovery window. It reports facts and signs a verdict. It cannot
  edit a plan, schedule a worker, make a product decision or run arbitrary commands.
- The **head orchestrator decides**. It owns scheduling, `WAIT` dependencies, replanning and user
  decision requests. It cannot invent a `CLEAR` verdict, expand a clearance manifest or bypass
  approval delivery.
- The **worker executes**. It receives one fenced assignment and may write only its exact allowed
  files and resources. It cannot enumerate other plans, change its own manifest, mint a
  clearance or directly contact an unrelated chat.

No component inherits another component's authority. Compromise or failure of one boundary must
not silently grant the other two.

## Fail-closed outcomes

`CLEAR`
: The census is complete and fresh, the normalized manifests do not conflict, and every input
  required to mint a clearance is present. This is the only claim-capable verdict.

`WAIT`
: The proposed work conflicts with an active owner but can safely proceed after a named lease or
  node completion. No worker claim is granted while waiting.

`REPLAN`
: A conflicting change invalidates an assumption, interface, schema, resource contract or file
  allocation. The orchestrator must produce a revised plan, obtain a fresh assessment and obtain
  user approval again when the change is material.

`USER_DECISION`
: More than one safe disposition exists, or the system cannot infer the user's intended scope.
  No automated choice or worker claim is permitted.

`UNKNOWN`
: Any required fact is absent, stale, malformed, unreachable, ambiguous, unsupported or outside
  the configured census. `UNKNOWN` is the default outcome; absence is never interpreted as
  freedom to proceed.

## Approval is a delivery transaction

Board approval alone never unlocks work. Each managed board is explicitly registered to one
originating chat before approval. The approval transition and its notification outbox record are
one idempotent transaction. Worker admission remains blocked while that notification is
`UNROUTED`, `QUEUED`, `CLAIMED`, retrying or `DEAD_LETTER`. Only a matching durable delivery
receipt for the registered route satisfies the approval input.

Repeated clicks, watcher restarts and connector retries may create multiple delivery attempts,
but they must resolve to one logical approval and one correlation identity. A route is never
guessed from a window title, process list, recently active chat or model-generated text.

## What the guarantee does not cover

The guarantee covers only Perfect Planner-managed workers because their claim path is mediated.
It cannot prevent arbitrary external editors, terminals, IDE extensions, legacy agents, users or
malware from modifying files outside that path. Those actors are detected through dirty-state,
Git reconciliation and release gates; they are not falsely described as prevented.

An unregistered legacy Planner, an offline repository, an unreachable board, an unsupported
registry version or an incomplete configured-root census forces `UNKNOWN` and zero managed worker
claims. The operator must register, recover or explicitly remove the unsupported participant
before a fresh assessment can return `CLEAR`.

The journal is tamper-evident rather than tamper-proof. Local CI mirrors hosted CI but cannot
guarantee identical external infrastructure. Visual evidence still requires human judgment.

## Required recovery behavior

- A crash before atomic clearance consumption leaves no worker claim.
- A crash after consumption recovers the exact fenced lease or expires it; it never remints from
  stale state.
- Registry-generation, manifest, branch, snapshot or fence drift revokes the clearance.
- Lost chat delivery retries from the durable outbox and keeps admission blocked.
- Ambiguous recovery becomes `UNKNOWN` and requires a new census.
