	.text
	.globl _main
_main:
	pushq %rbp
	movq %rsp, %rbp
	subq $64, %rsp
	movsd ___double_const_0(%rip), %xmm14
	movsd %xmm14, -8(%rbp)
	movsd -8(%rbp), %xmm14
	movsd %xmm14, -16(%rbp)
	movsd ___double_const_1(%rip), %xmm14
	movsd %xmm14, -24(%rbp)
	movsd -24(%rbp), %xmm14
	movsd %xmm14, -32(%rbp)
	movsd -16(%rbp), %xmm14
	movsd %xmm14, -40(%rbp)
	movsd -40(%rbp), %xmm14
	addsd -32(%rbp), %xmm14
	movsd %xmm14, -40(%rbp)
	movsd -40(%rbp), %xmm14
	movsd %xmm14, -48(%rbp)
	movsd -48(%rbp), %xmm14
	cvttsd2sil %xmm14, %r10d
	movl %r10d, -52(%rbp)
	movl -52(%rbp), %eax
	addq $64, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
	movl $0, %eax
	addq $64, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
	.data
	.balign 8
___double_const_0:
	.quad 4613937818241073152
	.data
	.balign 8
___double_const_1:
	.quad 4611686018427387904
