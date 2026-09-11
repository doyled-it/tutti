---
name: design-slice
description: Facilitates the Slice movement of a Tutti design session. Use after the Structure movement to map the backbone of the journey, cut the thinnest end-to-end walking-skeleton slice, and then prioritize the ribs that flesh it out. Asks one question at a time, may invoke the story-map diagram skill when the backbone reads better as a picture, and emits the movement artifact section once the backbone, the walking skeleton, and the prioritized ribs are covered.
---

# Slice movement

Find the thinnest thing that works end to end, then grow it. The guiding question is: what is
the thinnest end-to-end path, then the ribs? The walking skeleton is a slice that touches every
part of the system while doing almost nothing, so the whole path is proven before it is fleshed
out.

## What to cover

- **Backbone mapped**: the sequence of activities the user moves through, left to right.
- **Walking skeleton sliced**: the thinnest slice that runs the whole path end to end, even if
  each step is trivial.
- **Ribs prioritized**: the increments that flesh out each backbone step, ordered by value.

## How to facilitate

Ask ONE question at a time. Map the backbone first, then push for the thinnest slice that still
touches every step ("what is the least we could build and still go end to end?"), then order
the ribs by value rather than by convenience. Resist a first slice that is one fat feature
instead of a thin end-to-end path.

## Diagrams

When the backbone and its slices read better as a picture, invoke the `story_map` diagram skill
and embed its inline SVG in the artifact section.

## When you are done

Once the backbone, the walking skeleton, and the prioritized ribs are covered, emit the tagged
complete reply with the artifact section as markdown: a `## Slice` heading, the backbone, the
walking-skeleton slice, and the ribs in priority order.
