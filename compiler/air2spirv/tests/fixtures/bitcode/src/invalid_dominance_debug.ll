; Own test input (WP-SC-02): the invalid IR of invalid_dominance.ll plus current debug info
; ("Debug Info Version" 3). LLVM 18's reader verifies such modules itself during the debug-info
; auto-upgrade and calls report_fatal_error when the verifier fails; air2spirv disables that
; upgrade and must report A2S_ERROR_MALFORMED_INPUT instead of terminating.
; Assembled with `llvm-as-18 -disable-verify` by ../generate.sh.
target triple = "x86_64-unknown-linux-gnu"

define i32 @bad(i1 %c) !dbg !5 {
entry:
  br i1 %c, label %then, label %join, !dbg !8
then:
  %x = add i32 1, 2, !dbg !8
  br label %join, !dbg !8
join:
  ret i32 %x, !dbg !8
}

!llvm.dbg.cu = !{!0}
!llvm.module.flags = !{!2, !3}

!0 = distinct !DICompileUnit(language: DW_LANG_C11, file: !1, emissionKind: FullDebug)
!1 = !DIFile(filename: "invalid.c", directory: ".")
!2 = !{i32 7, !"Dwarf Version", i32 5}
!3 = !{i32 2, !"Debug Info Version", i32 3}
!4 = !DISubroutineType(types: !{})
!5 = distinct !DISubprogram(name: "bad", scope: !1, file: !1, line: 1, type: !4, unit: !0, spFlags: DISPFlagDefinition)
!8 = !DILocation(line: 2, column: 3, scope: !5)
