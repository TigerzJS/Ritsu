; Own test input (WP-SC-02): valid IR with debug info that claims "Debug Info Version" 2.
; LLVM 18 only reads version 3; air2spirv drops the debug info with a warning and continues.
; Assembled with `llvm-as-18 -disable-verify` by ../generate.sh.
target triple = "x86_64-unknown-linux-gnu"

define i32 @add(i32 %a, i32 %b) !dbg !5 {
entry:
  %s = add i32 %a, %b, !dbg !8
  ret i32 %s, !dbg !8
}

!llvm.dbg.cu = !{!0}
!llvm.module.flags = !{!2, !3}

!0 = distinct !DICompileUnit(language: DW_LANG_C11, file: !1, emissionKind: FullDebug)
!1 = !DIFile(filename: "old.c", directory: ".")
!2 = !{i32 7, !"Dwarf Version", i32 5}
!3 = !{i32 2, !"Debug Info Version", i32 2}
!4 = !DISubroutineType(types: !{})
!5 = distinct !DISubprogram(name: "add", scope: !1, file: !1, line: 1, type: !4, unit: !0, spFlags: DISPFlagDefinition)
!8 = !DILocation(line: 2, column: 3, scope: !5)
