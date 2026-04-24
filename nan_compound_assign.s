	.text
	.globl _main
_main:
	pushq %rbp
	movq %rsp, %rbp
	subq $112, %rsp
	movsd ___double_const_0(%rip), %xmm14
	movsd %xmm14, -8(%rbp)
	movsd -8(%rbp), %xmm14
	movsd %xmm14, -16(%rbp)
	movsd -16(%rbp), %xmm14
	divsd _zero.0(%rip), %xmm14
	movsd %xmm14, -16(%rbp)
	movsd -16(%rbp), %xmm14
	movsd %xmm14, -24(%rbp)
	movsd ___double_const_1(%rip), %xmm14
	movsd %xmm14, -32(%rbp)
	movsd -24(%rbp), %xmm14
	movsd %xmm14, -40(%rbp)
	movsd -40(%rbp), %xmm14
	addsd -32(%rbp), %xmm14
	movsd %xmm14, -40(%rbp)
	movsd -40(%rbp), %xmm14
	movsd %xmm14, -24(%rbp)
	movsd -24(%rbp), %xmm0
	call _double_isnan
	movl %eax, -44(%rbp)
	cmpl $0, -44(%rbp)
	movl $0, -48(%rbp)
	sete -48(%rbp)
	cmpl $0, -48(%rbp)
	je .Lif_end.0
	movl $1, %eax
	addq $112, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
.Lif_end.0:
	movsd -24(%rbp), %xmm14
	movsd %xmm14, -56(%rbp)
	movsd -56(%rbp), %xmm14
	subsd -24(%rbp), %xmm14
	movsd %xmm14, -56(%rbp)
	movsd -56(%rbp), %xmm14
	movsd %xmm14, -24(%rbp)
	movsd -24(%rbp), %xmm0
	call _double_isnan
	movl %eax, -60(%rbp)
	cmpl $0, -60(%rbp)
	movl $0, -64(%rbp)
	sete -64(%rbp)
	cmpl $0, -64(%rbp)
	je .Lif_end.1
	movl $2, %eax
	addq $112, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
.Lif_end.1:
	movsd ___double_const_2(%rip), %xmm14
	movsd %xmm14, -72(%rbp)
	movsd -24(%rbp), %xmm14
	movsd %xmm14, -80(%rbp)
	movsd -80(%rbp), %xmm14
	mulsd -72(%rbp), %xmm14
	movsd %xmm14, -80(%rbp)
	movsd -80(%rbp), %xmm14
	movsd %xmm14, -24(%rbp)
	movsd -24(%rbp), %xmm0
	call _double_isnan
	movl %eax, -84(%rbp)
	cmpl $0, -84(%rbp)
	movl $0, -88(%rbp)
	sete -88(%rbp)
	cmpl $0, -88(%rbp)
	je .Lif_end.2
	movl $3, %eax
	addq $112, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
.Lif_end.2:
	movsd ___double_const_3(%rip), %xmm14
	movsd %xmm14, -96(%rbp)
	movsd -24(%rbp), %xmm14
	movsd %xmm14, -104(%rbp)
	movsd -104(%rbp), %xmm14
	divsd -96(%rbp), %xmm14
	movsd %xmm14, -104(%rbp)
	movsd -104(%rbp), %xmm14
	movsd %xmm14, -24(%rbp)
	movsd -24(%rbp), %xmm0
	call _double_isnan
	movl %eax, -108(%rbp)
	cmpl $0, -108(%rbp)
	movl $0, -112(%rbp)
	sete -112(%rbp)
	cmpl $0, -112(%rbp)
	je .Lif_end.3
	movl $4, %eax
	addq $112, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
.Lif_end.3:
	movl $0, %eax
	addq $112, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
	movl $0, %eax
	addq $112, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
	.bss
	.balign 16
_zero.0:
	.zero 8
	.data
	.balign 16
___double_const_0:
	.quad 0
	.data
	.balign 16
___double_const_1:
	.quad 4636680996359294157
	.data
	.balign 16
___double_const_2:
	.quad 4616189618054758400
	.data
	.balign 16
___double_const_3:
	.quad 0
