#define SET_CONTEXT set_context
#define SWITCH_CONTEXT switch_context

.global SET_CONTEXT
.global SWITCH_CONTEXT

.text
.align 4

SET_CONTEXT:
        pop     %rbp                 /* # 1. スタックから %rbp を取り出して（関数のベースポインタを）復元 */
        xor     %eax, %eax           /* # 2. %eax を 0 にする（関数の戻り値を 0 に設定）*/

        movq    %rbx, (%rdi)         /* # 3. %rbx を %rdi が指すアドレスに保存 */
        movq    %rbp, 8(%rdi)        /* # 4. %rbp を %rdi+8 バイトの位置に保存 */
        movq    %r12, 16(%rdi)       /* # 5. %r12 を %rdi+16 バイトの位置に保存 */
        movq    %r13, 24(%rdi)       /* # 6. %r13 を %rdi+24 バイトの位置に保存 */
        movq    %r14, 32(%rdi)       /* # 7. %r14 を %rdi+32 バイトの位置に保存 */
        movq    %r15, 40(%rdi)       /* # 8. %r15 を %rdi+40 バイトの位置に保存 */

        lea     8(%rsp), %rdx        /* # 9. スタックポインタ %rsp + 8 を %rdx に格納 */
        movq    %rdx, 48(%rdi)       /* # 10. スタックポインタ (関数の呼び出し後の位置) を %rdi+48 に保存 */

        push    %rbp                 /* # 11. %rbp をスタックにプッシュして保存 */
        movq    (%rsp), %rdx         /* # 12. スタックのトップにある %rbp を %rdx にロード, rsp はこの関数を呼び出したものが「次に実行するべきアドレス」 */
        movq    %rdx, 56(%rdi)       /* # 13. それを %rdi+56 に保存（関数の戻りアドレスの保存） */
        ret                          /* # 14. 関数から復帰 */

.text
.align 4

SWITCH_CONTEXT:
        pop     %rsi                 /* # 1. 戻りアドレスを %rsi にロード */

        movq    (%rdi), %rbx         /* # 2. [ctx + 0] を %rbx にロード */
        movq    8(%rdi), %rbp        /* # 3. [ctx + 8] を %rbp にロード */
        movq    16(%rdi), %r12       /* # 4. [ctx + 16] を %r12 にロード */
        movq    24(%rdi), %r13       /* # 5. [ctx + 24] を %r13 にロード */
        movq    32(%rdi), %r14       /* # 6. [ctx + 32] を %r14 にロード */
        movq    40(%rdi), %r15       /* # 7. [ctx + 40] を %r15 にロード */

        movq    48(%rdi), %rdx       /* # 8. [ctx + 48] を %rsp にロード */
        movq    %rdx, %rsp           /* # 9. スタックポインタを復元 */
        addq    $0x8, %rsp           /* # 10. スタックを 8 バイト進める */

        push    %rbp                 /* # 11. %rbp をプッシュ */
        push    %rsi                 /* # 12. 戻りアドレス %rsi をプッシュ */

        movq    56(%rdi), %rdx       /* # 13. [ctx + 56] を %rdx にロード, [ctx + 56] は、Registers.rdx */
        jmpq    *%rdx                /* # 14. 関数のアドレスにジャンプ */