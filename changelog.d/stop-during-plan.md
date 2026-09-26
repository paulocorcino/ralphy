---
kind: fix
---
Stop now also stops a run while it is planning an issue. Before, the Claude
planner kept running until it finished its plan, and a stop during planning
could end the run with a planning error instead of as a stopped run.
