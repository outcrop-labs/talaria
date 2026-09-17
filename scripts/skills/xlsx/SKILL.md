---
name: xlsx
description: Spreadsheets are Talaria sheets (create_sheet), not local .xlsx files. Row 0 is the header. Do not reach for openpyxl or Excel.
---

# Spreadsheets (Talaria)

A tracker, a comparison, a grid someone will extend — `create_sheet`. Pass
`rows` as string[][]; row 0 is the header. Read it back with `get_document`.

Do not write a .xlsx to disk with openpyxl and call it done: that file dies
with the container. Do not dump a markdown table into `create_document` when
the ask was a spreadsheet. The playbook is the **talaria-toolkit** skill.
