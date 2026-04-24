	.data
	.globl _nested
	.balign 16
_nested:
	.long 1
	.long 2
	.long 3
	.long 4
	.long 5
	.long 6
	.long 7
	.long 8
	.long 9
	.long 10
	.long 11
	.long 12
	.long 13
	.long 14
	.long 15
	.long 16
	.long 17
	.long 18
	.long 19
	.long 20
	.long 21
	.long 22
	.long 23
	.long 24
	.long 25
	.long 26
	.long 27
	.long 28
	.long 29
	.long 30
	.text
	.globl _read
_read:
	pushq %rbp
	movq %rsp, %rbp
	subq $96, %rsp
	movq %rdi, -8(%rbp)
	movl %esi, -12(%rbp)
	movl %edx, -16(%rbp)
	movl %ecx, -20(%rbp)
	movslq -12(%rbp), %r10
	movq %r10, -32(%rbp)
	movq -8(%rbp), %rax
	movq -32(%rbp), %rdx
	imulq $60, %rdx
	leaq (%rax, %rdx, 1), %r10
	movq %r10, -40(%rbp)
	movq -40(%rbp), %r10
	movq %r10, -48(%rbp)
	movslq -16(%rbp), %r10
	movq %r10, -56(%rbp)
	movq -48(%rbp), %rax
	movq -56(%rbp), %rdx
	imulq $20, %rdx
	leaq (%rax, %rdx, 1), %r10
	movq %r10, -64(%rbp)
	movq -64(%rbp), %r10
	movq %r10, -72(%rbp)
	movslq -20(%rbp), %r10
	movq %r10, -80(%rbp)
	movq -72(%rbp), %rax
	movq -80(%rbp), %rdx
	leaq (%rax, %rdx, 4), %r10
	movq %r10, -88(%rbp)
	movq -88(%rbp), %r11
	movl (%r11), %r10d
	movl %r10d, -92(%rbp)
	movl -92(%rbp), %eax
	addq $96, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
	movl $0, %eax
	addq $96, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
	.text
	.globl _main
_main:
	pushq %rbp
	movq %rsp, %rbp
	subq $128, %rsp
	leaq _nested(%rip), %r10
	movq %r10, -8(%rbp)
	movq -8(%rbp), %rdi
	movl $1, %esi
	movl $1, %edx
	movl $0, %ecx
	call _read
	movl %eax, -12(%rbp)
	cmpl $21, -12(%rbp)
	movl $0, -16(%rbp)
	setne -16(%rbp)
	cmpl $0, -16(%rbp)
	je .Lif_end.0
	movl $1, %eax
	addq $128, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
.Lif_end.0:
	leaq _nested(%rip), %r10
	movq %r10, -24(%rbp)
	movq $1, -32(%rbp)
	movq -32(%rbp), %r10
	movq %r10, -40(%rbp)
	movq -40(%rbp), %r11
	imulq $60, %r11
	movq %r11, -40(%rbp)
	movq -24(%rbp), %r10
	movq %r10, -48(%rbp)
	movq -40(%rbp), %r10
	addq %r10, -48(%rbp)
	movq -48(%rbp), %r10
	movq %r10, -56(%rbp)
	movq $0, -64(%rbp)
	movq -56(%rbp), %rax
	movq -64(%rbp), %rdx
	leaq (%rax, %rdx, 8), %r10
	movq %r10, -72(%rbp)
	movq -72(%rbp), %r11
	movq (%r11), %r10
	movq %r10, -80(%rbp)
	movq $0, -88(%rbp)
	movq -80(%rbp), %rax
	movq -88(%rbp), %rdx
	leaq (%rax, %rdx, 4), %r10
	movq %r10, -96(%rbp)
	movq -96(%rbp), %r11
	movl (%r11), %r10d
	movl %r10d, -100(%rbp)
	movslq -100(%rbp), %r10
	movq %r10, -112(%rbp)
	movq $0, %rax
	movq -112(%rbp), %rdx
	leaq (%rax, %rdx, 4), %r10
	movq %r10, -120(%rbp)
	movq -120(%rbp), %r11
	movl (%r11), %r10d
	movl %r10d, -124(%rbp)
	cmpl $16, -124(%rbp)
	movl $0, -128(%rbp)
	setne -128(%rbp)
	cmpl $0, -128(%rbp)
	je .Lif_end.1
	movl $2, %eax
	addq $128, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
.Lif_end.1:
	movl $0, %eax
	addq $128, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
	movl $0, %eax
	addq $128, %rsp
	movq %rbp, %rsp
	popq %rbp
	ret
