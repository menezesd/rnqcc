	.text
	.globl _main
_main:
	pushq %rbp
	movq %rsp, %rbp
	subq $48, %rsp
	movl $3, -4(%rbp)
	leaq -4(%rbp), %r10
	movq %r10, -16(%rbp)
	movq -16(%rbp), %r10
	movq %r10, -24(%rbp)
	movq $0, -32(%rbp)
	movq -24(%rbp), %rax
	movq -32(%rbp), %rdx
	leaq (%rax, %rdx, 4), %r10
	movq %r10, -40(%rbp)
	movq -40(%rbp), %r11
	movl (%r11), %r10d
	movl %r10d, -44(%rbp)
	cmpl $3, -44(%rbp)
	movl $0, -48(%rbp)
	setne -48(%rbp)
	cmpl $0, -48(%rbp)
	je .Lif_end.0
	movl $1, %eax
	addq $48, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
.Lif_end.0:
	movl $0, %eax
	addq $48, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
	movl $0, %eax
	addq $48, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
