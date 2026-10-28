> 🧔 This doc was written by a human. LLMs are not allowed to edit this file.

# Ply Goals

Ply is built on the premise that LLM inference speeds will improve massively.

Fast LLMs will move the bottleneck from inference to tool use and verification.

Specifically, when coding with a fast LLM in existing languages, development
time will be dominated by:
* Slow build times
* Large and slow test suites
* Finding and fixing bugs
* Dealing with security issues

Ply is meant to be the perfect language to build performant and reliable
software extremely quickly when inference gets fast.

## Principles

### Incremental Everything

Whenever we make a change in ply, we should do the minimum amount of work
possible to build and test the change. That is, we must only rebuild what we
must and we must perfectly select and run only the tests that may actually be
affected by the change.

Another way to put this is that changing code in a ply project should cost
O(change) work rather than O(project) work. This applies to building, testing,
and verifying the correctness of the code.

#### How we do it

We store our code in a content-addressed cache that has all the information to
trace everything that can be affected by a change.

We keep most code pure and we track all side-effects any code can have. Our
effects system allows us to swap handlers for any effect which allows us to
write our tests in a deterministic manner which allows us to cache test results
safely and only rerun tests that actually might be impacted by a change.

### Signature-based development

The signature of each function and the shape of each type says everything
important about the code. Putting the types, the effects, and proved assertions
about the costs of a function, we can prevent most bugs that would otherwise
arise in code.

Putting everything in the function signatures makes it easier to design and
review code for humans and LLMs. For design, writing the types and function
signatures becomes enough to have a high degree of confidence in the
implementation. For review, huge changes become tractable to review because only
the types and function signatures need close scrutiny.

#### How we do it

We put types, effects, and costs in the function signature. Our effects are
fine-grained, so we can know exactly what a function is capable of. Putting
costs into the signature allows us to be confident the code is not
algorithmically incorrect (e.g., we can know that the function never allocates
or that it has O(n * log n) time complexity).

We can also be confident that the code only effects what we think it should
(e.g., the code cannot unexpectedly access the filesystem).

### Performant enough for the job

If ply isn't performant, then people will use other languages or integrate with
code from other languages for the hard parts of their codebase. Doing so would
undermine our fixes to all of the problems from the start of this doc.
Therefore, ply must be performant enough for most every programming job.

We aim to be as fast as languages like go and to be as fast as possible in as
many cases as it is feasible. Aside from building an operating system, speed
should never stop someone from using ply.

#### How we do it

Zero-cost abstractions, no overhead wherever possible. Verifiable cost clauses
in signatures to ensure that the implementations meet our performance
expectations.

### Deterministic testing

Testing in ply should be thorough, fast, and never flaky.

#### How we do it

Our effects system lets us replace non-deterministic parts of tests with
deterministic counterparts.

### Safe by construction

Making code safe by construction reduces the amount of time spent shipping and
dealing with bugs.

#### How we do it

Sum-types, pure functions, and any other patterns we can find that make it
easier to write correct code and harder to write incorrect code.

### Verifiable

LLMs make it more feasible to formally verify your code and ply must make it
feasible to do so in a fast development loop. Code can include laws that
constrain how the code operates. This makes the code more trustable and
reviewable because we can have proven invariants about our code. This helps
reduce the amount of time squashing bugs.

#### How we do it

We must include a proof system that is highly expressive and that can be run
quickly in alignment with our Incremental Everything principle.

### Supply Chain Safety

LLMs write lots of code that isn't all reviewed and LLMs are great at finding
exploits. Together this means our supply chains are a huge threat.


#### How we do it

Ply improves on supply chain security by being batteries included. Ply should
include a comprehensive standard library and a large set of blessed packages to
limit how many targets there are in the supply chain that can be compromised.
Furthermore, ply should limit what dependencies can do by leveraging its effects
as a capabilities system.
