// ─────────────────────────────────────────────────────────────────────────────
// template.typ — thesis document template
// ─────────────────────────────────────────────────────────────────────────────

#let thesis(
  title: [],
  subtitle: [],
  author: "",
  date: datetime.today(),
  abstract: [],
  acknowledgements: [],
  body,
) = {

  // ── Document metadata ──────────────────────────────────────────────────────
  set document(
    title: title,
    author: author,
  )

  // ── Page geometry ──────────────────────────────────────────────────────────
  set page(
    paper: "a4",
    margin: (top: 3cm, bottom: 3cm, inside: 3.5cm, outside: 2.5cm),
    binding: left,
  )

  // ── Typography ─────────────────────────────────────────────────────────────
  set text(font: "New Computer Modern", size: 11pt, lang: "en")
  set par(justify: true, leading: 0.65em)
  show heading: set block(above: 1.4em, below: 1em)

  // ── Code blocks ────────────────────────────────────────────────────────────
  show raw.where(block: true): it => block(
    fill: luma(245),
    inset: (x: 10pt, y: 8pt),
    radius: 4pt,
    width: 100%,
    text(font: "DejaVu Sans Mono", size: 9pt, it),
  )
  show raw.where(block: false): it => text(
    font: "DejaVu Sans Mono",
    size: 0.9em,
    it,
  )

  // ── Heading styles ─────────────────────────────────────────────────────────
  set heading(numbering: "1.1")

  show heading.where(level: 1): it => {
    pagebreak(weak: true)
    v(2em)
    text(size: 22pt, weight: "bold")[
      #counter(heading).display("1.") #h(0.5em) #it.body
    ]
    v(1.2em)
    line(length: 100%, stroke: 0.5pt + gray)
    v(0.8em)
  }

  show heading.where(level: 2): it => {
    v(1.2em)
    text(size: 14pt, weight: "bold")[
      #counter(heading).display("1.1") #h(0.4em) #it.body
    ]
    v(0.6em)
  }

  show heading.where(level: 3): it => {
    v(0.8em)
    text(size: 12pt, weight: "bold")[
      #counter(heading).display("1.1.1") #h(0.3em) #it.body
    ]
    v(0.4em)
  }

  // ── Figures & tables ───────────────────────────────────────────────────────
  set figure(gap: 0.8em)
  set figure.caption(separator: [ — ])
  show figure.caption: it => text(size: 9pt, style: "italic", it)

  // ── Lists ──────────────────────────────────────────────────────────────────
  set list(indent: 1em)
  set enum(indent: 1em)

  // ──────────────────────────────────────────────────────────────────────────
  // TITLE PAGE
  // ──────────────────────────────────────────────────────────────────────────
  page(
    margin: (top: 4cm, bottom: 3cm, left: 3cm, right: 3cm),
    {
      set align(center)

      v(1fr)

      text(size: 28pt, weight: "bold", title)
      v(0.8em)
      text(size: 16pt, style: "italic", fill: luma(60), subtitle)

      v(3em)
      line(length: 60%, stroke: 0.5pt)
      v(3em)

      text(size: 14pt, author)
      v(0.6em)
      text(size: 11pt, date.display("[month repr:long] [year]"))

      v(1fr)
    },
  )

  // ──────────────────────────────────────────────────────────────────────────
  // FRONT MATTER (roman page numbers)
  // ──────────────────────────────────────────────────────────────────────────
  set page(
    numbering: "i",
    number-align: center,
    header: [],
  )
  counter(page).update(1)

  // Abstract
  heading(outlined: false, numbering: none, level: 1, "Abstract")
  abstract
  pagebreak()

  // Acknowledgements
  heading(outlined: false, numbering: none, level: 1, "Acknowledgements")
  acknowledgements
  pagebreak()

  // Table of contents
  outline(title: "Table of Contents", indent: auto, depth: 3)
  pagebreak()

  // List of figures (only if figures exist)
  outline(title: "List of Figures", target: figure.where(kind: image))
  pagebreak()

  // List of listings
  outline(title: "List of Listings", target: figure.where(kind: raw))
  pagebreak()

  // ──────────────────────────────────────────────────────────────────────────
  // MAIN MATTER (arabic page numbers, running headers)
  // ──────────────────────────────────────────────────────────────────────────
  set page(
    numbering: "1",
    number-align: center,
    header: context {
      let page-num = counter(page).get().first()
      if calc.even(page-num) {
        let headings = query(selector(heading.where(level: 1)).before(here()))
        if headings.len() > 0 {
          let ch = headings.last()
          [#text(size: 9pt, fill: luma(80), ch.body) #box(width: 1fr)]
        }
      } else {
        let headings = query(selector(heading.where(level: 2)).before(here()))
        if headings.len() > 0 {
          let sec = headings.last()
          [#box(width: 1fr) #text(size: 9pt, fill: luma(80), sec.body)]
        }
      }
      line(length: 100%, stroke: 0.3pt + luma(180))
    },
  )
  counter(page).update(1)

  body
}
