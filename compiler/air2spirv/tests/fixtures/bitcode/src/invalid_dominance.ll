; Own test input (WP-SC-02): syntactically fine, but %x does not dominate its use, so
; llvm::verifyModule rejects it. Assembled with `llvm-as-18 -disable-verify` by ../generate.sh.
target triple = "x86_64-unknown-linux-gnu"

define i32 @bad(i1 %c) {
entry:
  br i1 %c, label %then, label %join
then:
  %x = add i32 1, 2
  br label %join
join:
  ret i32 %x
}
