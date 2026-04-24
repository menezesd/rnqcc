	.text
	.globl _main
_main:
	pushq %rbp
	movq %rsp, %rbp
	subq $144, %rsp
	movsd ___double_const_0(%rip), %xmm14
	movsd %xmm14, -8(%rbp)
	movsd -8(%rbp), %xmm14
	movsd %xmm14, -16(%rbp)
	movsd -16(%rbp), %xmm14
	divsd _zero.0(%rip), %xmm14
	movsd %xmm14, -16(%rbp)
	movsd -16(%rbp), %xmm14
	movsd %xmm14, -24(%rbp)
	movl $1, %r10d
	cvtsi2sdl %r10d, %xmm14
	movsd %xmm14, -32(%rbp)
	movsd -24(%rbp), %xmm14
	movsd %xmm14, -40(%rbp)
	movsd -40(%rbp), %xmm14
	addsd -32(%rbp), %xmm14
	movsd %xmm14, -40(%rbp)
	movsd -40(%rbp), %xmm14
	movsd %xmm14, -24(%rbp)
	movsd -40(%rbp), %xmm0
	call _double_isnan
	movl %eax, -44(%rbp)
	cmpl $0, -44(%rbp)
	movl $0, -48(%rbp)
	sete -48(%rbp)
	cmpl $0, -48(%rbp)
	je .Lif_end.0
	movl $1, %eax
	addq $144, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
.Lif_end.0:
	movl $1, %r10d
	cvtsi2sdl %r10d, %xmm14
	movsd %xmm14, -56(%rbp)
	movsd -24(%rbp), %xmm14
	movsd %xmm14, -64(%rbp)
	movsd -64(%rbp), %xmm14
	subsd -56(%rbp), %xmm14
	movsd %xmm14, -64(%rbp)
	movsd -64(%rbp), %xmm14
	movsd %xmm14, -24(%rbp)
	movsd -64(%rbp), %xmm0
	call _double_isnan
	movl %eax, -68(%rbp)
	cmpl $0, -68(%rbp)
	movl $0, -72(%rbp)
	sete -72(%rbp)
	cmpl $0, -72(%rbp)
	je .Lif_end.1
	movl $2, %eax
	addq $144, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
.Lif_end.1:
	movsd -24(%rbp), %xmm14
	movsd %xmm14, -80(%rbp)
	movl $1, %r10d
	cvtsi2sdl %r10d, %xmm14
	movsd %xmm14, -88(%rbp)
	movsd -24(%rbp), %xmm14
	movsd %xmm14, -96(%rbp)
	movsd -96(%rbp), %xmm14
	addsd -88(%rbp), %xmm14
	movsd %xmm14, -96(%rbp)
	movsd -96(%rbp), %xmm14
	movsd %xmm14, -24(%rbp)
	movsd -80(%rbp), %xmm0
	call _double_isnan
	movl %eax, -100(%rbp)
	cmpl $0, -100(%rbp)
	movl $0, -104(%rbp)
	sete -104(%rbp)
	cmpl $0, -104(%rbp)
	je .Lif_end.2
	movl $3, %eax
	addq $144, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
.Lif_end.2:
	movsd -24(%rbp), %xmm14
	movsd %xmm14, -112(%rbp)
	movl $1, %r10d
	cvtsi2sdl %r10d, %xmm14
	movsd %xmm14, -120(%rbp)
	movsd -24(%rbp), %xmm14
	movsd %xmm14, -128(%rbp)
	movsd -128(%rbp), %xmm14
	subsd -120(%rbp), %xmm14
	movsd %xmm14, -128(%rbp)
	movsd -128(%rbp), %xmm14
	movsd %xmm14, -24(%rbp)
	movsd -112(%rbp), %xmm0
	call _double_isnan
	movl %eax, -132(%rbp)
	cmpl $0, -132(%rbp)
	movl $0, -136(%rbp)
	sete -136(%rbp)
	cmpl $0, -136(%rbp)
	je .Lif_end.3
	movl $4, %eax
	addq $144, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
.Lif_end.3:
	movl $0, %eax
	addq $144, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
	movl $0, %eax
	addq $144, %rsp
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
