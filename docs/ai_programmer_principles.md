# AI Programmer Agent Development Principles

## Core Development Principles

This document outlines the key development principles that an AI programming agent must follow to ensure high-quality, maintainable, and efficient code.

### DRY (Don't Repeat Yourself)

**Implementation Rules:**
- Before writing new code, analyze the existing codebase for similar functionality
- If you find repetitive code (more than 3 lines), extract it into a separate function/method
- Create reusable components and utilities instead of copying code
- Use configuration files for repeated settings and constants
- Implement inheritance and composition patterns to avoid code duplication
- When refactoring, prioritize eliminating duplication over other improvements

**Validation Checklist:**
- [ ] Is there existing code that performs similar functionality?
- [ ] Can common patterns be extracted into utility functions?
- [ ] Are configuration values centralized and not hardcoded?
- [ ] Are similar data structures using shared interfaces or base classes?

### KISS (Keep It Simple, Stupid)

**Implementation Rules:**
- Choose the simplest solution that works and meets requirements
- Avoid premature optimization and complex architectural patterns without clear necessity
- Prefer readability over performance unless performance is critical
- Break complex functions into several simple ones (max 50 lines per function)
- Use clear, descriptive variable and function names
- Minimize cognitive complexity (cyclomatic complexity ≤ 10)
- Favor composition over inheritance when possible

**Validation Checklist:**
- [ ] Can this logic be simplified without losing functionality?
- [ ] Is the code self-documenting through clear naming?
- [ ] Are complex algorithms necessary or can simpler ones suffice?
- [ ] Can nested conditions be reduced or eliminated?

### YAGNI (You Aren't Gonna Need It)

**Implementation Rules:**
- Implement only features that are needed right now
- Don't add "future possibilities" without concrete requirements
- Remove unused code, functions, and dependencies regularly
- Avoid over-engineering solutions for hypothetical scenarios
- Focus on current user stories and requirements
- Resist adding configuration options "just in case"

**Validation Checklist:**
- [ ] Is every implemented feature currently required?
- [ ] Are all dependencies actively used?
- [ ] Is the current architecture sufficient for existing requirements?
- [ ] Can any code be removed without affecting functionality?

## Additional Core Principles

### SOLID Principles

**Single Responsibility Principle (SRP):**
- Each class/function should have only one reason to change
- Separate concerns into different modules/classes

**Open/Closed Principle (OCP):**
- Code should be open for extension but closed for modification
- Use interfaces and abstract classes for extensibility

**Liskov Substitution Principle (LSP):**
- Derived classes must be substitutable for their base classes
- Maintain behavioral compatibility in inheritance hierarchies

**Interface Segregation Principle (ISP):**
- Clients shouldn't depend on interfaces they don't use
- Create focused, specific interfaces

**Dependency Inversion Principle (DIP):**
- Depend on abstractions, not concretions
- Use dependency injection for loose coupling

### Code Quality Standards

**Naming Conventions:**
- Use descriptive, pronounceable names
- Avoid abbreviations and single-letter variables (except for loops)
- Use consistent naming patterns throughout the project
- Boolean variables should be questions (isValid, hasPermission)

**Function Design:**
- Functions should do one thing well
- Maximum 3-4 parameters per function
- Use pure functions when possible (no side effects)
- Return early to reduce nesting

**Error Handling:**
- Fail fast and provide meaningful error messages
- Use specific exception types
- Don't ignore or suppress errors silently
- Log errors with sufficient context

## Pre-Commit Validation Checklist

Before committing any code, verify:

### Code Quality
- [ ] **DRY**: No code duplication exists
- [ ] **KISS**: Logic is as simple as possible
- [ ] **YAGNI**: Only required features are implemented
- [ ] Code follows project style guidelines
- [ ] All functions have clear, single responsibilities

### Testing & Documentation
- [ ] New code is covered by tests (minimum 80% coverage)
- [ ] Complex logic includes unit tests
- [ ] Public APIs are documented
- [ ] README is updated if necessary

### Performance & Security
- [ ] No obvious performance bottlenecks
- [ ] Security best practices are followed
- [ ] Input validation is implemented where needed
- [ ] No sensitive data is hardcoded

### Dependencies & Architecture
- [ ] No unnecessary dependencies added
- [ ] Architecture supports current requirements without over-engineering
- [ ] Interfaces are well-defined and minimal
- [ ] Code is properly modularized

## Context-Aware Application

### Prototyping Phase
- Relax DRY principle in favor of speed
- Focus on proving concepts quickly
- Technical debt is acceptable for rapid iteration
- Prioritize working software over perfect architecture

### Production Code
- Strict adherence to all principles
- Comprehensive testing required
- Code reviews mandatory
- Performance and security are critical

### Legacy System Maintenance
- Make careful, incremental changes
- Respect existing architectural decisions
- Avoid large refactoring without clear business value
- Maintain backward compatibility

## Automated Quality Gates

Implement the following automated checks:

### Code Metrics
- **Cyclomatic Complexity**: ≤ 10 per function
- **Function Length**: ≤ 50 lines
- **Code Duplication**: ≤ 5%
- **Test Coverage**: ≥ 80%

### Static Analysis
- Linting rules enforcement
- Security vulnerability scanning
- Dependency vulnerability checks
- Code style consistency validation

### Performance Benchmarks
- Build time monitoring
- Runtime performance regression tests
- Memory usage validation
- API response time checks

## Continuous Improvement

### Regular Reviews
- Weekly code quality metrics review
- Monthly architecture assessment
- Quarterly principle adherence evaluation
- Annual development process optimization

### Learning & Adaptation
- Stay updated with industry best practices
- Adapt principles based on project experience
- Document lessons learned and pattern improvements
- Share knowledge across development teams

---

**Remember**: These principles are guidelines to help create better software. Apply them judiciously based on context, and always prioritize delivering value to users while maintaining code quality and team productivity.