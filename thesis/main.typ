// ─────────────────────────────────────────────────────────────────────────────
// main.typ — thesis entry point
// ─────────────────────────────────────────────────────────────────────────────

#import "template.typ": thesis

#show: thesis.with(
  title: [Simulation and Visualisation of Vehicular Network Routing Protocols],
  subtitle: [Design, Implementation and Evaluation of a Linux-native OBU/RSU Simulator in Rust],
  author: "Gonçalo Gomes",
  date: datetime.today(),
  abstract: include "chapters/00-abstract.typ",
  acknowledgements: [
    I would like to thank everyone who supported this work.
  ],
)

// ── Chapters ──────────────────────────────────────────────────────────────────

#include "chapters/01-introduction.typ"
#include "chapters/02-background.typ"
#include "chapters/03-architecture.typ"
#include "chapters/04-implementation.typ"
#include "chapters/05-security.typ"
#include "chapters/06-evaluation.typ"
#include "chapters/07-conclusion.typ"

// ── Bibliography ──────────────────────────────────────────────────────────────

#pagebreak()
#bibliography("bibliography.bib", style: "ieee", title: "References")
