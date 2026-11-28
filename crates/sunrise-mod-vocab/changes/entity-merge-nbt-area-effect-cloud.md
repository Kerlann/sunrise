# `EntityRef::merge_nbt` reloads a resident area effect cloud

`EntityRef::merge_nbt` now reloads a resident area effect cloud from the merged data, as `/data merge entity` does, instead of refusing it. Other entity kinds are unchanged (#6769). Nothing to change in a mod.
